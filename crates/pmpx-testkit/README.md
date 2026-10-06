# pmpx-testkit

Test helpers for plugin authors

A plugin's `PackageManager::command` is a pure function, so it can be tested without a project, a host or a process. `pmpx-testkit` is the three ways of doing that: `context()` builds a `Context` by hand, `Fixture` writes a real directory and runs the real detection over it, and `capture()` installs a fake host so a test can assert the lines a plugin wrote.

## API

- `context() -> pmpx_plugin::ContextBuilder` — the builder a plugin's own tests use.
- `Fixture::new()` — one project on a real temporary directory.
- `Fixture::file(name, contents)`, `Fixture::plugin(name, family, strong, weak)`, `Fixture::pin(family, plugin)`, `Fixture::prefer(families)`, `Fixture::declares(names)` — describe that project.
- `Fixture::path()`, `Fixture::context() -> Context<'static>` — where it is, and what the host would hand over.
- `capture() -> Captured` — installs the fake host, dropping whatever was recorded before.
- `Captured::take()`, `Captured::take_levels()` — drain the recorded lines.
- `Captured::level(u32)`, `Captured::trace()`, `Captured::normal()`, `Captured::info()`, `Captured::quiet()` — pick the level to capture.

## Notes

- It is meant for plugin authors' `[dev-dependencies]`; nothing in this workspace depends on it.
- The fixture runs the real `pmpx_detect::select`, so a test fails when a plugin's declared markers stop matching.
- `Fixture::declares(...)` is the allowlist: a file the test wrote but did not declare never reaches the context.
- Panicking is part of the interface — there is no error type, and a fixture that detects nothing panics with the listing.

Tests: unit tests in `src/fixture.rs` and `src/host.rs`; there is no `tests/` directory.
License: same as the workspace (see the repository root).