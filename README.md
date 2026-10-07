# pmpx

[![Checks](https://github.com/pmpx-rs/pmpx/actions/workflows/ci.yaml/badge.svg)](https://github.com/pmpx-rs/pmpx/actions/workflows/ci.yaml)
[![Release](https://github.com/pmpx-rs/pmpx/actions/workflows/release.yaml/badge.svg)](https://github.com/pmpx-rs/pmpx/actions/workflows/release.yaml)
[![crates.io](https://img.shields.io/crates/v/pmpx.svg)](https://crates.io/crates/pmpx)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](#license)
[![MSRV](https://img.shields.io/badge/rust-1.88%2B-blue.svg)](#requirements)

> 中文版见 [README_CN.md](README_CN.md)

One command surface for a project's package managers. `pmpx` detects what a project is,
translates a fixed set of verbs, and spawns the real tool — with **zero backends built into
the binary**.

```console
$ pmpx install serde            # a Rust project    → cargo add serde
$ pmpx install lodash           # a frontend repo   → pnpm add lodash
$ pmpx test -- --nocapture      #                   → cargo test --nocapture
$ pmpx exec tsc --noEmit        #                   → pnpm exec tsc --noEmit

$ pmpx                          # what is this directory, and which tool will run?
```

## Features

- 🎯 **Proxies your package manager** — the same commands work in every project, translated
  to whatever tool that project actually uses.
- 🔍 **Detects the project for you** — reads the files a repo already has. Nothing to
  initialise, no config file to write.
- 🔌 **A real plugin system** — every backend is a plugin, `dlopen`ed at runtime. The binary
  ships none, and a plugin need not share your rustc.
- 🔀 **Resolvable and overridable** — ties break on a priority array you edit; `.pmpx.toml`
  pins one backend per project.
- 🪂 **An escape hatch** — when a backend has no answer, `exec` falls back to running the
  command itself.

## Contents

- [Install](#install)
- [Updating](#updating)
- [Usage](#usage)
- [How it picks a backend](#how-it-picks-a-backend)
- [Configuration](#configuration)
- [What it does not do](#what-it-does-not-do)
- [Writing a plugin](#writing-a-plugin)
- [Repository](#repository)
- [License](#license)

## Install

```bash
cargo install pmpx
```

The binary is named `pmpx`.

Every release also attaches prebuilt archives, for Linux and Windows on x86_64 and for both
macOS architectures. Two ways to use them:

```console
# The install script downloads the archive for this machine, verifies it against the
# release's SHA256SUMS, and installs it. One file is written; PATH and shell profiles
# are left alone.
$ curl -LsSf https://raw.githubusercontent.com/pmpx-rs/pmpx/main/install.sh | sh

# Or, with cargo-binstall
$ cargo binstall pmpx
```

On Windows the script is `install.ps1`:

```console
$ irm https://raw.githubusercontent.com/pmpx-rs/pmpx/main/install.ps1 | iex
```

Both install to `~/.local/bin` (Unix) or `%USERPROFILE%\.pmpx\bin` (Windows), and both take
`PMPX_INSTALL_DIR`, `PMPX_VERSION` and `PMPX_BASE_URL` (a mirror) from the environment. A
platform with no archive is not a failure — `cargo install pmpx` builds from source anywhere
Rust does.

### Requirements

- Rust **1.88+** to build.
- **At least one plugin.** `pmpx` detects nothing without one — that is deliberate, see
  [Writing a plugin](#writing-a-plugin).

## Updating

```console
$ pmpx self update --check         # what is available; changes nothing
$ pmpx self update                 # replace this binary with the newest release
$ pmpx self update --version 0.1.0 # or a specific one, which is also how you roll back
```

Only installations that cargo does not manage can do this. `cargo install` keeps its own
record of what it put on disk, so a pmpx installed that way is told to run
`cargo install pmpx --force` instead — replacing the file behind cargo's back would make
`cargo install --list` disagree with reality. The record is what decides, not the directory:
a `cargo binstall` or a hand-unpacked archive sits in the same place and does update itself.

The download is checked against the release's `SHA256SUMS` before anything is replaced; a
mismatch stops there and leaves the running binary alone. That is **not a signature**: the
checksum file and the archive come from the same place, so it proves the bytes arrived
intact, not who built them.

Nothing is ever checked automatically — no background check, no "a new version is
available" banner on startup.

## Usage

| Verb | Aliases | No arguments | With arguments |
| ---- | ------- | ------------ | -------------- |
| `install` | `i` `add` | install from the lockfile | add a dependency |
| `remove` | `rm` `uninstall` | — | remove a dependency |
| `run` | `r` | — | run a script / target |
| `build` | `b` | build | forwarded to the backend |
| `test` | `t` | test | forwarded to the backend |
| `update` | `up` | update dependencies | update the named ones |
| `exec` | `x` | — | escape hatch, see below |

Everything after `--` is passed through untouched:

```console
$ pmpx test -- --nocapture
$ pmpx run dev -- --port 3000
```

Global flags:

| Flag | Meaning |
| ---- | ------- |
| `-p, --plugin <name>` | use this plugin, **overriding `.pmpx.toml`** |
| `-C, --dir <path>` | operate in this directory |
| `--no-walk-up` | only look at the current directory |
| `-q, --quiet` | suppress the hints and the resolved command on stderr |
| `--debug` | print debug information for this run |
| `--json` | print the run as JSON on stdout, one object per line (see below) |
| `--explain` | report how the backend was chosen, and run nothing |

Other commands: `pmpx info`, `pmpx plugin ls|current|set|unset|add|rm|update|search|info`,
`pmpx config get|set`, `pmpx completion <shell>`, `pmpx self update`.

### Exit codes

| Code | Meaning |
| ---- | ------- |
| 0 | success |
| 1 | `pmpx` itself failed, **or the plugin panicked** |
| 2 | usage error, or the backend does not support that verb |
| 3 | no project detected, no plugin for it, or the tool is not on `PATH` |
| **N** | **the backend's own exit code, passed through** |

The last row is the point: `pmpx test` has to fail when the tests fail.

### `exec`

```
pmpx exec <cmd...>
   ├─ plugin supports exec     → its answer (npx, pnpm exec, …)
   └─ it does not, or no project → pmpx runs the command itself
```

So `pmpx exec ls` works with zero plugins installed. Every other verb reports an error
instead — an explicit exception, not silent magic.

### `--json`

```console
$ pmpx --json build | jq -c 'select(.event == "finished")'
{"code":0,"event":"finished"}
```

One JSON object per line (JSONL) on **stdout**, so a program can read a run as it happens:
`resolved`, `starting`, `finished`, `phase` (the engine's own timings), `warning`, `note`, `error`,
`notes`, `plugin`. Everything meant for a person — including the backend's own output — goes to
**stderr**, and that is what keeps the stream parseable: under `--json` the backend writes to stderr
instead of stdout. Without the flag nothing changes — output stays live, colours stay, and
`pmpx build > log` means what it always meant.

`--json` covers the verbs that run something. A command whose result is a table (say `pmpx plugin
ls`) refuses the flag and exits 2 rather than mixing prose into the stream.

## How it picks a backend

Detection **loads no plugin code**. It scores installed plugins against the files in the
project root, using the patterns each plugin declares in its manifest:

| Evidence | Score | Meaning |
| -------- | ----- | ------- |
| `strong` | 100 | a lockfile, workspace file or shape marker — **this backend was actually used** |
| `weak` | 10 | a manifest file — **this only proves the ecosystem** |
| pinned in `.pmpx.toml` | 50 (floor) | "I wrote `node = "pnpm"`, so this is a Node project" |

Then it picks a family, then a plugin inside it. Ties are broken by two arrays you control:

```toml
# <pmpx-config-dir>/config.toml
[plugin]
family_priority = ["node", "rust", "python", "go", "jvm", "dotnet", "php", "ruby"]
priority        = ["pnpm", "npm", "yarn", "bun", "cargo"]
```

**"Mixed projects default to Node" is not a hardcoded rule** — it is the default
`family_priority` falling out. Move `rust` to the front and the same project goes to `cargo`.

This is also the known failure mode: a library crate that gitignores `Cargo.lock` scores 10
from `Cargo.toml`, ties with `package.json`, and loses to Node. That is the unavoidable cost
of a two-tier score — **without a lockfile you genuinely cannot tell**. The way out is a pin.

## Configuration

| What | Where |
| ---- | ----- |
| Global | `<config-dir>/pmpx/config.toml` — Linux `~/.config/pmpx`, macOS `~/Library/Application Support/pmpx`, Windows `%APPDATA%\pmpx\config` |
| Plugins | `~/.pmpx/plugins/` on all three platforms |
| Per project | `.pmpx.toml`, **any number of them**, collected upwards from the cwd |

Nearer `.pmpx.toml` wins, so a monorepo can set a default at the root and override it in a
package. `pmpx plugin set <name>` writes the nearest one.

```toml
# .pmpx.toml
[plugin]
node = "pnpm"      # pin Node to pnpm, and give Node a 50-point floor
rust = "cargo"
```

Two environment variables override the paths, for tests and for multi-environment setups:
`PMPX_CONFIG_DIR`, `PMPX_DATA_DIR`.

## What it does not do

**`pmpx` only writes its own files.** `~/.pmpx/**`, the global config, and — when you ask for
it — a project's `.pmpx.toml`. There is **no cache file in your project**: a cache would save
under a millisecond and cost you a dirty working tree.

Never touched, on purpose:

```text
~/.cargo/config.toml   ~/.npmrc   ~/.yarnrc   ~/.yarnrc.yml   ~/.bunfig.toml
~/.config/pip/*        ~/.gemrc   src/**     *.lock
```

**No proxy, mirror or registry feature at all** — not even reading them. Configure those in
your shell or in each tool's own config; `pmpx` inherits the environment as-is when it spawns.

## Writing a plugin

A plugin implements one trait and adds one line:

```rust
use std::ffi::OsString;
use pmpx_plugin::{CommandSpec, Context, Family, PackageManager, PluginError, Verb};

struct CargoPlugin;

impl PackageManager for CargoPlugin {
    fn name(&self) -> &str { "cargo" }
    fn family(&self) -> Family { Family::RUST }

    fn command(&self, ctx: &Context, verb: Verb, args: &[OsString])
        -> Result<CommandSpec, PluginError>
    {
        match verb {
            Verb::Install if args.is_empty() => Ok(CommandSpec::new("cargo").arg("fetch")),
            Verb::Install => Ok(CommandSpec::new("cargo").arg("add").args(args.iter())),
            // say so rather than guessing — the host degrades `exec` for you
            Verb::Exec => Err(PluginError::unsupported_verb(verb)),
            other => Err(PluginError::other(format!("not implemented: {other}"))),
        }
    }
}

/// The factory the wrapper calls.
pub fn create() -> Box<dyn PackageManager> { Box::new(CargoPlugin) }
```

A plugin crate is a **plain rlib**: no `#[no_mangle]`, no `crate-type`. When pmpx installs (or packs)
a plugin it generates a few-line wrapper that calls `pmpx_plugin::export!(create)` and builds *that*
into the cdylib — so the whole C ABI shim (`catch_unwind`, string lifetimes, the capability tables) is
generated, and a plugin author never sees any of it. The rlib shape is also what makes `cargo test`
work on the plugin directly.

**`command()` is a pure mapping.** Its inputs are the verb, the arguments, and what the host knows
about the call. It may not read files, write files, read the environment, or run processes — which
is why shape decisions go through `Context::matched`:

```rust
// Yarn classic and Berry spell this one verb differently, and the only evidence is a file.
Verb::Update if ctx.has_matched(".yarnrc.yml") => Ok(CommandSpec::new("yarn").arg("up")),
Verb::Update => Ok(CommandSpec::new("yarn").arg("upgrade")),
```

When a *name* is not enough, a plugin declares what it wants to read **in its own manifest**, and the
host reads exactly that and hands over the bytes — still without interpreting them:

```toml
[context]
files = ["package.json", ".yarnrc.yml"]
```

```rust
// The plugin parses it; pmpx never learns what is inside.
if ctx.file_str("package.json").is_some_and(|s| s.contains("\"packageManager\"")) { … }
```

The context also carries what only the host knows: `start_dir` (where the person ran pmpx — the only
way to tell which package of a monorepo this is, and **not** where the command will run), `reason`
and `score` (why this plugin was picked), and `pins`, `scripts` and `config_files` (the project
config as it was read).

**A plugin explains itself through the host.** `pmpx_plugin::debug!` / `info!` / `warn!` /
`error!` — and `debug::context()`, which prints the whole context on one line — reach pmpx, which
adds the plugin's id and decides what to print:

```
pmpx debug: [pnpm] context: root=… start=… matched=[package.json pnpm-lock.yaml] verb=install …
```

That switch lives on the host side, so `--debug` never becomes an input a plugin could branch on: a
debug run executes byte-for-byte the same command as any other. With no host — a plugin's own
`cargo test` — the macros fall back to stderr, so the author still sees them.

The contract crate is [`pmpx-plugin`](https://crates.io/crates/pmpx-plugin) —
**zero dependencies**, MSRV 1.82.

## Repository

<https://github.com/pmpx-rs/pmpx>

| Path | What |
| ---- | ---- |
| `crates/pmpx/` | the host binary |
| `crates/pmpx-plugin/` | the plugin contract: trait, C ABI, `export!` |
| `crates/pmpx-plugin/tests/` | unit tests, ABI shim tests, and a real `dlopen` end-to-end |
| `crates/pmpx/tests/` | end-to-end tests that run the built binary against a real `cdylib` plugin |

Each backend lives in its own repository (`pmpx-plugin-cargo`, `pmpx-plugin-pnpm`, …) and is
published separately.

```bash
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo package -p pmpx-plugin --locked
```

## License

MIT — see [LICENSE](LICENSE).
