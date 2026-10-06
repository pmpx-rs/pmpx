# pmpx-plugin-abi

The raw C ABI between pmpx and a plugin

The wire format both sides of the `dlopen` boundary compile against: `#[repr(C)]` data, the numbers that go with it, the context keys a host may answer, and the capabilities each side may provide. It depends on nothing, stays `no_std`, and is hand-translatable, so a plugin written in C, Zig or Go is a supported target. The safe surface a plugin author writes against is `pmpx-plugin`, and the host side is `pmpx-loader`. Its `rust-version` is 1.82, the same floor as `pmpx-plugin`, because its MSRV decides who can write a plugin.

## API

- `PmpxStr` / `PmpxSlice<T>` — borrowed byte and slice views.
- `PmpxContext`, `PmpxHost`, `PmpxPlugin`, `PmpxCommand`, `PmpxCommandCap`, `PmpxIdentity`, `PmpxAttach`, `PmpxLog` — the tables, plus `RunFn` for `PmpxCommandCap::run`.
- `PMPX_ABI_MAJOR`, `PMPX_ENTRY_SYMBOL`, `PMPX_MAX_ITEMS`, `PMPX_OK`, `PMPX_ERR_*`, `PMPX_VERB_*`, `PMPX_REASON_*`, `PMPX_LEVEL_*` — the numbers.
- `PMPX_KEY_*` and `PMPX_CAP_*` — the key and capability names, plus the `PMPX_KEYS`, `PMPX_CAPS` and `PMPX_REQUIRED_CAPS` lists.
- `surface::write_snapshot` / `surface::write_c_header` — the renderers behind `abi/surface.toml` and `include/pmpx_plugin.h`.
- `bytes_to_os` / `os_to_bytes` — behind the `std` feature, which is off by default.

## Notes

- Absent and empty are different: `ptr == null` means the host has nothing under that key (not an error), while `len == 0` with a non-null `ptr` is an empty value.
- Memory is freed by the side that allocated it, a borrowed view lasts only for the call it arrived with, and a panic must not cross `extern "C"`.
- Unknown is not an error in either direction: an unknown key answers absent, and a plugin must answer `PMPX_ERR_UNSUPPORTED_VERB` for a verb it does not know.
- `file.<name>` is answered only for names the plugin declared in its `[context] files`; every table is `size`-prefixed, and a reader never reads past the `size` the other side wrote.

Tests: `cargo test -p pmpx-plugin-abi` — unit tests in `src/`, comparing both committed renderings through `include_str!`; no C compiler and no filesystem needed.
License: same as the workspace (see the repository root).