//! Inject the compile-time rustc version and target triple for `pmpx_plugin::abi` to fill into
//! `PmpxPluginV1`.
//!
//! `rustc --version` is not something that can be asked at runtime, so ask once at compile time
//! and pin the answer into the binary. These constants get linked into every plugin, so a plugin
//! does not need a build.rs of its own.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Switching toolchains has to re-run this, otherwise the previous toolchain's version number
    // gets pinned.
    println!("cargo:rerun-if-env-changed=RUSTC");

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());

    // Write "unknown" when it cannot be asked -- this field is only for diagnostics, and failing
    // to get it should not break the build.
    let version = Command::new(&rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=PMPX_BUILD_RUSTC={version}");
    println!("cargo:rustc-env=PMPX_BUILD_TARGET={target}");
}
