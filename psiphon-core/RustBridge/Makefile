# Builds the cgo bridge used by the Rust TUI as a shared library.
#
# Usage:
#   make            # build libpsiphon_bridge.so + libpsiphon_bridge.h here
#   make clean

OUT := libpsiphon_bridge.so
HDR := libpsiphon_bridge.h

.PHONY: shared clean
shared: $(OUT)

$(OUT): bridge.go
	CGO_ENABLED=1 go build -buildmode=c-shared -o $(OUT) bridge.go

clean:
	rm -f $(OUT) $(HDR)
