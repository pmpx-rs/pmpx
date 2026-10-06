# pmpx-loader

The host side of the plugin ABI

`pmpx-loader` opens a plugin cdylib, negotiates the capability tables it offers (`identity`, `command`, and the optional `attach`) and calls it. It is one of the workspace's designated `unsafe` islands (the CI guard `unsafe stays in the ABI islands` lists them all). Everything a plugin reads through the context is copied out for that one call and released once afterwards.

## API

- `unsafe fn Plugin::open(path: &Path) -> Result<Plugin, LoadError>` — the whole load path; `Plugin` keeps the library alive.
- `Plugin::tables()`, `Plugin::name()`, `Plugin::family()` — accessors on the loaded plugin.
- `Tables` — the same tables without the library handle: `unsafe Tables::from_root(...)`, `build_info()`, `attach()`, `call()`.
- `Command { program, args, cwd }` — the plugin's answer, already copied into owned Rust values; `cwd: None` means the project root.
- `ContextSource<'a>` — everything the host answers for one call; `Files::contents(name)` is asked at most once per name.
- `NoFiles` — a `Files` that offers nothing.
- `LoadError` — `Open`, `NoEntrySymbol`, `Major`, `MissingCapability`, `ShortTable`.
- `CallError` — `UnsupportedVerb`, `InvalidArgs`, `Internal`, `Unknown(u32)`, `ShortCommand`.

## Notes

- Finding the library file is the store's job (`crate_plugin_kit::find_library`); loading it is this crate's — `Plugin::open` never searches.
- Strings and commands are copied across the boundary, and the plugin's `free_str` / `free_command` runs once; nothing borrowed from the plugin outlives its call.
- A missing capability or an unknown return code is reported as a typed error rather than guessed; `abi_major` is compared with `PMPX_ABI_MAJOR`.
- `Tables` and `Plugin` hold a raw pointer, so neither is `Send` or `Sync`.

Tests: unit tests against hand-written tables, plus `tests/dlopen.rs` loading a real cdylib fixture.
License: same as the workspace (see the repository root).