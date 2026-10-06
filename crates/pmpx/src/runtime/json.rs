//! The machine-readable side of what happened: one JSON object per line, on stdout.
//!
//! The same events the human renderer shows, for a program instead of an eye. JSONL rather than one
//! array, because a script wants each event as it arrives (a build takes minutes) and `jq` reads it
//! line by line either way.
//!
//! Only stdout carries JSON. Everything meant for a person -- the `pmpx -> ...` announcement, notes,
//! warnings, and the backend's own output -- goes to stderr, so a caller can pipe stdout into a parser
//! without filtering prose out of it first.

use pmpx_engine::{Event, ProgramKind};
use serde_json::{json, Value};

/// One event, as a JSON object.
pub fn event(plugin: &str, event: &Event) -> Value {
    match event {
        Event::Resolved {
            program,
            path,
            kind,
        } => json!({
            "event": "resolved",
            "program": program.to_string_lossy(),
            "path": path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "kind": kind_name(*kind),
        }),

        Event::Starting { plan } => json!({
            "event": "starting",
            "program": plan.program.to_string_lossy(),
            "args": plan
                .args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            "cwd": plan.cwd.as_ref().map(|d| d.to_string_lossy().into_owned()),
        }),

        Event::Finished { code } => json!({ "event": "finished", "code": code }),

        Event::Warning(text) => json!({ "event": "warning", "text": text }),
        Event::Note(text) => json!({ "event": "note", "text": text }),
        Event::Error(text) => json!({ "event": "error", "text": text }),

        Event::Phase {
            name,
            micros,
            detail,
        } => json!({
            "event": "phase",
            "name": name,
            "micros": micros,
            "detail": detail,
        }),

        Event::Notes { plugin, notes } => json!({
            "event": "notes",
            "plugin": plugin,
            "notes": notes,
        }),

        Event::PluginMessage { level, text } => json!({
            "event": "plugin",
            "plugin": plugin,
            "level": level,
            "text": text,
        }),
    }
}

/// How a program will be started, spelled for a reader of the JSON.
fn kind_name(kind: ProgramKind) -> &'static str {
    match kind {
        ProgramKind::Native => "native",
        ProgramKind::CmdShim => "cmd",
        ProgramKind::PowerShellShim => "powershell",
    }
}
