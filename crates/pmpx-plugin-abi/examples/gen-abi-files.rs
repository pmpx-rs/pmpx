//! Regenerate the two committed renderings of the ABI.
//!
//! ```text
//! cargo run -p pmpx-plugin-abi --example gen-abi-files            # write both files
//! cargo run -p pmpx-plugin-abi --example gen-abi-files -- --check # fail if they are stale
//! ```
//!
//! Run it after touching anything in this crate. `--check` is what CI runs, so a wire-format change
//! cannot slip in without the generated files being updated in the same commit.

use std::path::PathBuf;
use std::process::ExitCode;

use pmpx_plugin_abi::surface::{write_c_header, write_snapshot};

/// How one file is rendered.
type Render = fn(&mut String) -> std::fmt::Result;

fn main() -> ExitCode {
    let check = std::env::args().any(|arg| arg == "--check");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let files: [(PathBuf, Render); 2] = [
        (root.join("abi").join("surface.toml"), |out| {
            write_snapshot(out)
        }),
        (root.join("include").join("pmpx_plugin.h"), |out| {
            write_c_header(out)
        }),
    ];

    let mut stale = Vec::new();

    for (path, render) in files {
        let mut text = String::new();
        if let Err(e) = render(&mut text) {
            eprintln!("cannot render {}: {e}", path.display());
            return ExitCode::FAILURE;
        }

        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current == text {
            println!("{} is up to date", path.display());
            continue;
        }

        if check {
            stale.push(path);
            continue;
        }

        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("cannot create {}: {e}", parent.display());
                return ExitCode::FAILURE;
            }
        }
        if let Err(e) = std::fs::write(&path, &text) {
            eprintln!("cannot write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        println!("wrote {}", path.display());
    }

    if stale.is_empty() {
        return ExitCode::SUCCESS;
    }

    for path in stale {
        eprintln!(
            "::error::{} is out of date -- run \
             `cargo run -p pmpx-plugin-abi --example gen-abi-files` and review the diff",
            path.display()
        );
    }
    ExitCode::FAILURE
}
