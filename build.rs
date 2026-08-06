// Builds the vendored Go cgo bridge (psiphon-core/RustBridge) into a shared
// library, then links this crate against it and makes sure the .so ends up
// next to the final binary so it can be found at runtime.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let bridge_dir = manifest_dir.join("psiphon-core/RustBridge");
    let bridge_go = bridge_dir.join("bridge.go");
    let lib_name = "psiphon_bridge";
    let so_name = format!("lib{lib_name}.so");
    let built_so = bridge_dir.join(&so_name);

    println!("cargo:rerun-if-changed={}", bridge_go.display());

    // GOPROXY: the default proxy.golang.org / dl.google.com module+toolchain
    // hosts are unreachable in some sandboxed environments; goproxy.cn is a
    // reliable public mirror. Respect a user-supplied GOPROXY if set.
    let goproxy = env::var("GOPROXY").unwrap_or_else(|_| "https://goproxy.cn,direct".to_string());

    let status = Command::new("go")
        .current_dir(&bridge_dir)
        .env("GOPROXY", goproxy)
        .env("GOSUMDB", env::var("GOSUMDB").unwrap_or_else(|_| "sum.golang.org".to_string()))
        .env("CGO_ENABLED", "1")
        .args([
            "build",
            "-buildmode=c-shared",
            "-o",
            &so_name,
            "bridge.go",
        ])
        .status()
        .expect("failed to invoke `go build` — is Go installed and on PATH?");

    if !status.success() {
        panic!("go build of the psiphon bridge failed (see output above)");
    }

    // Copy the freshly built .so into OUT_DIR and link against it there,
    // then also drop a copy next to the final executable so it can be
    // found at runtime without requiring the user to set LD_LIBRARY_PATH.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let out_so = out_dir.join(&so_name);
    std::fs::copy(&built_so, &out_so).expect("failed to copy bridge .so into OUT_DIR");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=dylib={lib_name}");

    // Make the binary find libpsiphon_bridge.so next to itself at runtime.
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");

    copy_to_target_dir(&built_so, &so_name);
}

/// Best-effort copy of the .so next to the built executable (target/debug or
/// target/release), so `cargo run` / running the binary directly both work
/// without extra setup.
fn copy_to_target_dir(built_so: &Path, so_name: &str) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    // OUT_DIR looks like <target_dir>/<profile>/build/<crate>-<hash>/out
    if let Some(target_profile_dir) = out_dir
        .ancestors()
        .find(|p| p.join("build").exists() && p.file_name().is_some())
    {
        let dest = target_profile_dir.join(so_name);
        let _ = std::fs::copy(built_so, &dest);
    }
}
