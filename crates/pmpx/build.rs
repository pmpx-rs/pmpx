//! Injects the target triple at compile time, for the release asset names to use.
//!
//! The same thing `crate-plugin-kit`'s build script does for itself, for the same reason: an asset name
//! needs the whole triple (`x86_64-pc-windows-msvc`), while `std::env::consts` only has the two halves
//! (`windows`, `x86_64`). `pmpx` reads its own rather than the kit's so that `self update` -- which has
//! nothing to do with the plugin store -- does not depend on the store's dependency.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=PMPX_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
