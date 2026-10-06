# pmpx

The CLI binary: argv in, a plugin's answer executed, output rendered

`pmpx` is the CLI binary of the project: it parses `argv`, opens one `pmpx_engine::Session`, runs one verb through it, and renders the engine's `Event`s to the terminal. It owns no detection, no plugin loading and no process spawning — the engine does all of that. Install and usage are documented once in the repository root [`README.md`](../../README.md).

## API

- `cli` — the `clap` surface: `Cli` and the subcommand table; argv into a struct, no semantics
- `app` — this host's view of one run: `options`, `session`, `select`, `load_backend`, `run_verb`, `run_script`, `explain`, `Sink`
- `commands` — subcommand handling: `dispatch`, plus `info`, `plugin`, `plugin_pin`, `plugin_store`, `config`
- `runtime` — presenting what the engine reports: `set_json`, `render_event`, `render`, `json`
- `style` — terminal styling; escape codes are written here and nowhere else
- `selfupdate` — `pmpx self update`: `release`, `checksum`, `archive`, `swap`, `ledger`, `version`
- `debug` — `--debug`: one dim line per phase, on stderr
- `error` — `PmpxError` and the exit-code mapping: `EXIT_OK`, `EXIT_USAGE`, `EXIT_NOT_FOUND`, `EXIT_INTERNAL`

## Notes

- Global flags (they work after a subcommand too): `--json` — stdout is JSON, one object per line, with the backend's output on stderr; `--explain`; `-p, --plugin <NAME>`; `-C, --dir <PATH>`; `--no-walk-up`; `-q, --quiet`; `--debug`.
- Verbs: `install` (`i`, `add`), `remove` (`rm`, `uninstall`), `run` (`r`), `build` (`b`), `test` (`t`), `update` (`up`), `exec` (`x`); plus `info`, `plugin`, `config`, `completion <shell>` and `self update`.
- `pmpx run <name>` resolves the project's `[scripts]` table before any plugin is asked, so a project can have `pmpx fmt` with no plugin at all.
- This crate owns terminal output: the engine prints nothing, and its `Event`s become the lines here.

Tests: the end-to-end suite in `crates/pmpx/tests/cli/` runs the built binary (`CARGO_BIN_EXE_pmpx`) against a temporary sandbox.
License: same as the workspace (see the repository root).