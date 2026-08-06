/*
 * RustBridge/bridge.go
 *
 * A small cgo shared-library shim around the psiphon-tunnel-core client
 * engine, written for this project (psiphon-rs-tui). It is NOT part of
 * upstream Psiphon-Labs/psiphon-tunnel-core.
 *
 * Unlike ClientLibrary/PsiphonTunnel.go (the stock C library, which blocks
 * for the whole connection attempt and never surfaces notices to the
 * caller), this bridge:
 *   - starts the tunnel asynchronously (PsiphonStart returns as soon as the
 *     config has been loaded and validated, not once connected),
 *   - streams every notice (the same JSON events every Psiphon client
 *     - Android/iOS/Windows/ConsoleClient - uses to report connection
 *     progress, ports, homepages, errors, etc.) out through a queue that the
 *     Rust side polls, so the TUI can render live status instead of a single
 *     blocking "did it work" result.
 *
 * It intentionally mirrors the setup/teardown sequence used by
 * ConsoleClient/main.go (OpenDataStore -> import embedded server list ->
 * NewController -> controller.Run) rather than reusing
 * ClientLibrary/clientlib, because clientlib's StartTunnel() is synchronous
 * and swallows notices.
 *
 * Exported C API:
 *
 *   int  PsiphonStart(const char *configPath, const char *serverListPath, const char *dataRootDirectory, const char *egressRegion);
 *        Loads and commits the config synchronously, then launches the
 *        tunnel in the background. Returns 0 on success, or a negative
 *        error code (see below) if the config could not even be loaded.
 *        A human-readable "BridgeError" notice is also pushed to the queue
 *        in that case. egressRegion is an optional ISO 3166-1 alpha-2
 *        country code (e.g. "US"); NULL or "" means no region preference
 *        (config.EgressRegion, if any, is left as-is).
 *
 *   void PsiphonStop(void);
 *        Cancels a running tunnel and blocks (bounded) until shutdown has
 *        completed. Safe to call even if nothing is running.
 *
 *   char *PsiphonPollNotice(int timeoutMs);
 *        Waits up to timeoutMs milliseconds (0 = don't block) for the next
 *        queued notice and returns it as a NUL-terminated JSON string, or
 *        NULL if none arrived in time. The caller must free the returned
 *        pointer with PsiphonFreeString.
 *
 *   void PsiphonFreeString(char *s);
 *        Frees a string returned by PsiphonPollNotice.
 *
 *   int  PsiphonIsRunning(void);
 *        Returns 1 if a tunnel is currently starting/running, 0 otherwise.
 */

package main

/*
#include <stdlib.h>
*/
import "C"

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
	"time"
	"unsafe"

	"github.com/Psiphon-Labs/psiphon-tunnel-core/psiphon"
)

const (
	startOK                     C.int = 0
	startErrAlreadyRunning      C.int = -1
	startErrConfigPathRequired  C.int = -2
	startErrConfigReadFailed    C.int = -3
	startErrConfigParseFailed   C.int = -4
	startErrConfigCommitFailed  C.int = -5
	startErrDataRootDirRequired C.int = -6
)

// noticeQueueCapacity bounds how many notices we buffer between Go and
// Rust. Rust polls in a tight loop on its own thread, so this is really
// just a cushion against brief scheduling delays, not a real backlog.
const noticeQueueCapacity = 1024

// bridgeState holds all mutable state for the (single) tunnel this bridge
// manages. There is intentionally only ever one active tunnel at a time,
// matching the CLI's own -config/-serverList/-dataRootDirectory model.
type bridgeState struct {
	mu        sync.Mutex
	running   bool
	noticeCh  chan []byte
	cancel    context.CancelFunc
	doneCh    chan struct{}
}

var state bridgeState

// pushRaw enqueues a raw notice payload (already JSON) onto ch without
// blocking. If the queue is full, the oldest entry is dropped to make room
// - live status beats stale backlog.
func pushRaw(ch chan []byte, notice []byte) {
	if ch == nil {
		return
	}
	// Defensive copy: the byte slice handed to us by psiphon's notice
	// writer is not guaranteed to remain valid/unmodified after the
	// callback returns.
	cp := make([]byte, len(notice))
	copy(cp, notice)

	select {
	case ch <- cp:
		return
	default:
	}
	// Queue full: drop the oldest entry, then try once more.
	select {
	case <-ch:
	default:
	}
	select {
	case ch <- cp:
	default:
	}
}

// pushSynthetic enqueues a bridge-generated event using the same
// {"noticeType","timestamp","data"} shape as real psiphon notices, so the
// Rust side can treat them uniformly. noticeType is prefixed with "Bridge"
// to keep it unambiguous from upstream notice types.
func pushSynthetic(ch chan []byte, noticeType string, data map[string]interface{}) {
	if data == nil {
		data = map[string]interface{}{}
	}
	event := map[string]interface{}{
		"noticeType": noticeType,
		"timestamp":  time.Now().UTC().Format(time.RFC3339Nano),
		"data":       data,
	}
	encoded, err := json.Marshal(event)
	if err != nil {
		return
	}
	pushRaw(ch, encoded)
}

//export PsiphonStart
func PsiphonStart(cConfigPath, cServerListPath, cDataRootDirectory, cEgressRegion *C.char) C.int {
	state.mu.Lock()
	if state.running {
		// Report rejection on whatever channel is currently active (the
		// still-running tunnel's), so the caller gets visible feedback
		// instead of silent failure.
		ch := state.noticeCh
		state.mu.Unlock()
		pushSynthetic(ch, "BridgeError", map[string]interface{}{
			"message": "a tunnel is already running or still shutting down; wait for it to stop first",
		})
		return startErrAlreadyRunning
	}
	// Still holding state.mu here (only the running-tunnel branch above
	// unlocks early) - released below, after state.noticeCh is set, so
	// there's no window where a concurrent call could see running == false
	// with a stale noticeCh.

	configPath := ""
	if cConfigPath != nil {
		configPath = C.GoString(cConfigPath)
	}
	serverListPath := ""
	if cServerListPath != nil {
		serverListPath = C.GoString(cServerListPath)
	}
	dataRootDirectory := ""
	if cDataRootDirectory != nil {
		dataRootDirectory = C.GoString(cDataRootDirectory)
	}
	egressRegion := ""
	if cEgressRegion != nil {
		egressRegion = C.GoString(cEgressRegion)
	}

	noticeCh := make(chan []byte, noticeQueueCapacity)
	state.noticeCh = noticeCh
	state.mu.Unlock()

	if configPath == "" {
		pushSynthetic(noticeCh, "BridgeError", map[string]interface{}{
			"message": "configPath is required (-config)",
		})
		return startErrConfigPathRequired
	}
	if dataRootDirectory == "" {
		pushSynthetic(noticeCh, "BridgeError", map[string]interface{}{
			"message": "dataRootDirectory is required (-dataRootDirectory)",
		})
		return startErrDataRootDirRequired
	}

	// Route every psiphon notice (regular + diagnostic) into our queue.
	// This must happen before LoadConfig so that config-loading errors,
	// which are themselves emitted as notices, are captured too.
	err := psiphon.SetNoticeWriter(psiphon.NewNoticeReceiver(func(notice []byte) {
		pushRaw(noticeCh, notice)
	}))
	if err != nil {
		pushSynthetic(noticeCh, "BridgeError", map[string]interface{}{
			"message": "failed to set notice writer: " + err.Error(),
		})
		return startErrConfigCommitFailed
	}

	configJSON, err := os.ReadFile(configPath)
	if err != nil {
		psiphon.SetEmitDiagnosticNotices(true, false)
		psiphon.NoticeError("error loading configuration file: %s", err)
		return startErrConfigReadFailed
	}

	config, err := psiphon.LoadConfig(configJSON)
	if err != nil {
		psiphon.SetEmitDiagnosticNotices(true, false)
		psiphon.NoticeError("error processing configuration file: %s", err)
		return startErrConfigParseFailed
	}

	config.DataRootDirectory = dataRootDirectory
	// Keep pre-DataRootDirectory-era file locations pinned to the same
	// directory, matching ClientLibrary/clientlib's behaviour.
	config.MigrateDataStoreDirectory = dataRootDirectory
	config.MigrateObfuscatedServerListDownloadDirectory = dataRootDirectory
	config.MigrateRemoteServerListDownloadFilename = filepath.Join(dataRootDirectory, "server_list_compressed")

	if egressRegion != "" {
		config.EgressRegion = egressRegion
	}

	// Force on diagnostic notices so the TUI has rich, real-time status to
	// show (candidate servers being tried, obfuscation layers, etc.). We do
	// NOT set config.UseNoticeFiles, so these notices flow straight through
	// our notice writer above instead of being redirected to rotating log
	// files.
	config.EmitDiagnosticNotices = true

	err = config.Commit(true)
	if err != nil {
		psiphon.SetEmitDiagnosticNotices(true, false)
		psiphon.NoticeError("error committing configuration: %s", err)
		return startErrConfigCommitFailed
	}

	ctx, cancel := context.WithCancel(context.Background())
	doneCh := make(chan struct{})

	state.mu.Lock()
	state.running = true
	state.cancel = cancel
	state.doneCh = doneCh
	state.mu.Unlock()

	go runTunnel(ctx, config, serverListPath, noticeCh, doneCh)

	return startOK
}

// runTunnel performs the actual connection sequence in the background,
// mirroring ConsoleClient's TunnelWorker. It always closes doneCh on exit
// so PsiphonStop can wait on it.
func runTunnel(
	ctx context.Context,
	config *psiphon.Config,
	serverListPath string,
	noticeCh chan []byte,
	doneCh chan struct{}) {

	// Defers run LIFO. close(doneCh) is what unblocks PsiphonStop's caller
	// (and thus, transitively, whoever is waiting to call PsiphonStart
	// again), so it must be the very last thing that happens - it's listed
	// first here so everything below, including ResetNoticeWriter, has
	// already completed by the time it fires.
	defer close(doneCh)

	// SetNoticeWriter (called on every PsiphonStart) errors out if a writer
	// is already set, and it is only ever cleared here. Forgetting this, or
	// running it after close(doneCh), means a start immediately following a
	// stop can fail with "notice writer already set".
	defer psiphon.ResetNoticeWriter()

	defer func() {
		state.mu.Lock()
		state.running = false
		state.cancel = nil
		state.mu.Unlock()
		pushSynthetic(noticeCh, "BridgeStopped", nil)
	}()

	pushSynthetic(noticeCh, "BridgeStarting", map[string]interface{}{
		"dataRootDirectory": config.DataRootDirectory,
	})

	err := psiphon.OpenDataStore(config)
	if err != nil {
		pushSynthetic(noticeCh, "BridgeError", map[string]interface{}{
			"message": "failed to open data store: " + err.Error(),
		})
		return
	}
	defer psiphon.CloseDataStore()

	// If specified, the embedded server list is loaded and stored. When
	// there are no server candidates at all, we wait for this import to
	// complete before starting the controller. Otherwise we import while
	// concurrently starting the controller, to minimize delay before
	// attempting to connect to existing candidate servers.
	var importWaitGroup sync.WaitGroup
	if serverListPath != "" {
		importWaitGroup.Add(1)
		go func() {
			defer importWaitGroup.Done()
			err := psiphon.ImportEmbeddedServerEntries(ctx, config, serverListPath, "")
			if err != nil {
				psiphon.NoticeError("error importing embedded server entry list: %s", err)
			}
		}()
		if !psiphon.HasServerEntries() {
			psiphon.NoticeInfo("awaiting embedded server entry list import")
			importWaitGroup.Wait()
		}
	}
	defer importWaitGroup.Wait()

	controller, err := psiphon.NewController(config)
	if err != nil {
		pushSynthetic(noticeCh, "BridgeError", map[string]interface{}{
			"message": "failed to create controller: " + err.Error(),
		})
		return
	}

	// Blocks until ctx is cancelled (via PsiphonStop) or the controller
	// exits on its own.
	controller.Run(ctx)
}

//export PsiphonStop
func PsiphonStop() {
	state.mu.Lock()
	cancel := state.cancel
	doneCh := state.doneCh
	state.mu.Unlock()

	if cancel == nil {
		return
	}
	cancel()

	if doneCh == nil {
		return
	}
	select {
	case <-doneCh:
	case <-time.After(15 * time.Second):
		// Give up waiting; the goroutine will still finish shutting down
		// in the background and clear state.running when it does.
	}
}

//export PsiphonPollNotice
func PsiphonPollNotice(timeoutMs C.int) *C.char {
	state.mu.Lock()
	ch := state.noticeCh
	state.mu.Unlock()

	if ch == nil {
		return nil
	}

	if timeoutMs <= 0 {
		select {
		case notice := <-ch:
			return C.CString(string(notice))
		default:
			return nil
		}
	}

	select {
	case notice := <-ch:
		return C.CString(string(notice))
	case <-time.After(time.Duration(timeoutMs) * time.Millisecond):
		return nil
	}
}

//export PsiphonFreeString
func PsiphonFreeString(s *C.char) {
	if s != nil {
		C.free(unsafe.Pointer(s))
	}
}

//export PsiphonIsRunning
func PsiphonIsRunning() C.int {
	state.mu.Lock()
	running := state.running
	state.mu.Unlock()
	if running {
		return 1
	}
	return 0
}

// main is a stub required by cgo when building with -buildmode=c-shared.
func main() {}
