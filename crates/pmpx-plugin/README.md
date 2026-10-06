# pmpx-plugin

The contract a plugin author implements

The contract a plugin author implements: the `PackageManager` trait and the shell that carries it safely across the `dlopen` boundary. A plugin crate is a plain rlib whose `pub fn create() -> Box<dyn PackageManager>` is the entry point the generated wrapper calls. The crate has no feature flags, depends only on `pmpx-plugin-abi`, and keeps `rust-version = "1.82"` because that MSRV decides who can write a plugin.

## API

- `PackageManager` — implement `name`, `family` and `command`.
- `Context<'a>` — `project_root`, `start_dir`, `matched`, `config_files`, `pins`, `reason`, `score`, and the methods `has_matched`, `was_pinned`, `pinned_for`, `file`, `file_str`.
- `CommandSpec` — `new(program)`, `arg`, `args`, `cwd`, with the public `program`, `args` and `cwd` fields.
- `Verb` — the closed set `Install`, `Remove`, `Update`, `Run`, `Build`, `Test`, `Exec`, with `ALL`, `to_abi`, `from_abi` and `as_str`.
- `Family` — an open string type with the constants `NODE`, `RUST`, `PYTHON`, `GO`, `JVM`, `DOTNET`, `PHP`, `RUBY` and `new(name)`.
- `PluginError` — `UnsupportedVerb`, `InvalidArgs`, `Other`, with `unsupported_verb`, `invalid_args`, `other`, `code()` and `message()`.
- `debug!` / `info!` / `warn!` / `error!` — the logging macros, plus the `debug` module (`Level`, `wants`, `emit`, `context`).
- `export!`, `shell` and `abi` — the macro that generates the ABI, the plugin-side helpers (`dispatch`, `guard`, `PANIC_MARKER`, `BUILD_RUSTC`, `BUILD_TARGET`) and the wire format.

## Notes

- `export!` goes in the wrapper crate that becomes the cdylib, not in the plugin crate: writing it in both defines `pmpx_plugin_entry_v3` twice and fails at link time.
- `command()` is a pure mapping — no file reads, no environment, no child processes, no network; project contents arrive only through `Context::matched` and the declared `[context] files`, and an undeclared name is never readable.
- A panic in `command` becomes `PMPX_ERR_INTERNAL` and one in `name` or `family` becomes `PANIC_MARKER`; the trait is synchronous, with no `async` and no callbacks.
- The crate requires `std` (only `pmpx-plugin-abi` is `no_std`), and `Context::builder()` builds a context by hand for a plugin's own tests.

Tests: `cargo test -p pmpx-plugin` — unit tests beside the code, plus `tests/in_process.rs`, `tests/short_log_table.rs` and `tests/dlopen.rs`, which builds `tests/fixtures/toy-plugin` into a cdylib and loads it.
License: same as the workspace (see the repository root).