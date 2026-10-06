# pmpx-engine

Runs what a plugin answers, and reports it as values

`pmpx-engine` owns the execution half of `pmpx`: a plugin answers *what* to run, and this crate decides *how* — resolving the real path, choosing the interpreter, inheriting stdio, waiting, translating the exit status. The pipeline is open → select → load → run: `Session::open`, `select`, `load_backend`/`load_plugin`, then `flow::run_verb` and `run`. It is the only crate in the workspace that builds a `std::process::Command`, and it never prints: a run is reported as `Event`s.

## API

- `Plan` — this crate's own "what to run": `program`, `args`, `cwd`
- `Session` — one run's state, read once in `Session::open`; `Options` carries what argv said
- `Event` — the whole report of a run, including `Phase { name, micros, detail }`
- `ChildOutput` — where a started program's output goes: `Inherit`, or `OnStderr` for `--json`
- `run` — resolve, spawn, wait, pass the exit code through; needs no plugin
- `resolve` / `ProgramKind` — the real path, and whether it is `Native`, `CmdShim` or `PowerShellShim`
- `store` feature (off by default) — `install`, `uninstall`, `update`, `search`, `view` and the installed plugin set; turning it on is what pulls in `crate-plugin-kit`

## Notes

- Without `store`, the `detect`, `session`, `flow` and `store` modules are not compiled; what is left is the execution core, with no `crate-plugin-kit` linked.
- A backend's exit code is passed through verbatim — `Ok(code)`, never `Err`; a code that does not fit in 8 bits becomes its low 8 bits, plus a `Warning`.
- It measures its own phases and reports each as `Event::Phase`, so a trace can attribute the time that is not the backend's runtime.
- The decision lives in `pmpx-detect` and the ABI in `pmpx-loader`; this crate is not depended on by them.

Tests: unit tests beside the code, plus `cargo test -p pmpx-engine --features store` for the store.
License: same as the workspace (see the repository root).