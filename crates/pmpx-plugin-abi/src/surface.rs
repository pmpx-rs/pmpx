//! Renderings of this ABI: a machine-checked snapshot, and a C header.
//!
//! Both are **generated from the Rust definitions** and committed, so that a change to the wire
//! format cannot happen quietly:
//!
//! - `abi/surface.toml` is the ABI's surface -- keys, capabilities, numbers, and the size, alignment
//!   and field offsets of every struct. A test compares it with what this module renders, so any
//!   change has to be written into the file in the same commit and reviewed as a diff. The
//!   classification rule (what is additive and what needs [`crate::PMPX_ABI_MAJOR`] to move) is in
//!   `docs/refactor.md` §3.3.
//! - `include/pmpx_plugin.h` is the same ABI for a plugin written in C, Zig or Go. It carries the
//!   numbers as `_Static_assert`s, so a C compiler checks the layout this module computed.
//!
//! Regenerate both with:
//!
//! ```text
//! cargo run -p pmpx-plugin-abi --example gen-abi-files
//! cargo run -p pmpx-plugin-abi --example gen-abi-files -- --check   # CI: fail instead of write
//! ```
//!
//! If a struct grows a field, its size changes, the assertions below fire, and both the spec table
//! in this file and the C header have to be updated by hand -- which is the point: the numbers are
//! derived, but the *shape* is reviewed.

use core::fmt::{self, Write};
use core::mem::{align_of, offset_of, size_of};

use crate::caps::{PMPX_CAPS, PMPX_REQUIRED_CAPS};
use crate::keys::PMPX_KEYS;
use crate::types::{
    PmpxAttach, PmpxCommand, PmpxCommandCap, PmpxContext, PmpxHost, PmpxIdentity, PmpxLog,
    PmpxPlugin, PmpxSlice, PmpxStr, PMPX_ABI_MAJOR, PMPX_ERR_INTERNAL, PMPX_ERR_INVALID_ARGS,
    PMPX_ERR_UNSUPPORTED_VERB, PMPX_LEVEL_DEBUG, PMPX_LEVEL_ERROR, PMPX_LEVEL_INFO,
    PMPX_LEVEL_WARN, PMPX_MAX_ITEMS, PMPX_OK, PMPX_REASON_EXPLICIT, PMPX_REASON_PINNED,
    PMPX_REASON_SCORED, PMPX_VERB_BUILD, PMPX_VERB_EXEC, PMPX_VERB_INSTALL, PMPX_VERB_REMOVE,
    PMPX_VERB_RUN, PMPX_VERB_TEST, PMPX_VERB_UPDATE,
};

/// One field of a struct, and where it starts.
struct Field {
    name: &'static str,
    offset: usize,
}

/// One `#[repr(C)]` struct: its size, and every field with its offset.
struct Spec {
    name: &'static str,
    size: usize,
    align: usize,
    fields: &'static [Field],
}

const fn field(name: &'static str, offset: usize) -> Field {
    Field { name, offset }
}

/// Every struct that crosses the boundary, in the order the renderings list them.
const SPECS: &[Spec] = &[
    Spec {
        name: "PmpxStr",
        size: size_of::<PmpxStr>(),
        align: align_of::<PmpxStr>(),
        fields: &[
            field("ptr", offset_of!(PmpxStr, ptr)),
            field("len", offset_of!(PmpxStr, len)),
        ],
    },
    Spec {
        name: "PmpxSlice<PmpxStr>",
        size: size_of::<PmpxSlice<PmpxStr>>(),
        align: align_of::<PmpxSlice<PmpxStr>>(),
        fields: &[
            field("ptr", offset_of!(PmpxSlice<PmpxStr>, ptr)),
            field("len", offset_of!(PmpxSlice<PmpxStr>, len)),
        ],
    },
    Spec {
        name: "PmpxContext",
        size: size_of::<PmpxContext>(),
        align: align_of::<PmpxContext>(),
        fields: &[
            field("size", offset_of!(PmpxContext, size)),
            field("verb", offset_of!(PmpxContext, verb)),
            field("reason", offset_of!(PmpxContext, reason)),
            field("score", offset_of!(PmpxContext, score)),
            field("count", offset_of!(PmpxContext, count)),
            field("get", offset_of!(PmpxContext, get)),
            field("name", offset_of!(PmpxContext, name)),
        ],
    },
    Spec {
        name: "PmpxHost",
        size: size_of::<PmpxHost>(),
        align: align_of::<PmpxHost>(),
        fields: &[
            field("abi_major", offset_of!(PmpxHost, abi_major)),
            field("size", offset_of!(PmpxHost, size)),
            field("capability", offset_of!(PmpxHost, capability)),
        ],
    },
    Spec {
        name: "PmpxPlugin",
        size: size_of::<PmpxPlugin>(),
        align: align_of::<PmpxPlugin>(),
        fields: &[
            field("abi_major", offset_of!(PmpxPlugin, abi_major)),
            field("rustc_version", offset_of!(PmpxPlugin, rustc_version)),
            field("target", offset_of!(PmpxPlugin, target)),
            field("capability", offset_of!(PmpxPlugin, capability)),
        ],
    },
    Spec {
        name: "PmpxCommand",
        size: size_of::<PmpxCommand>(),
        align: align_of::<PmpxCommand>(),
        fields: &[
            field("size", offset_of!(PmpxCommand, size)),
            field("program", offset_of!(PmpxCommand, program)),
            field("args", offset_of!(PmpxCommand, args)),
            field("cwd", offset_of!(PmpxCommand, cwd)),
        ],
    },
    Spec {
        name: "PmpxIdentity",
        size: size_of::<PmpxIdentity>(),
        align: align_of::<PmpxIdentity>(),
        fields: &[
            field("size", offset_of!(PmpxIdentity, size)),
            field("name", offset_of!(PmpxIdentity, name)),
            field("family", offset_of!(PmpxIdentity, family)),
            field("free_str", offset_of!(PmpxIdentity, free_str)),
        ],
    },
    Spec {
        name: "PmpxCommandCap",
        size: size_of::<PmpxCommandCap>(),
        align: align_of::<PmpxCommandCap>(),
        fields: &[
            field("size", offset_of!(PmpxCommandCap, size)),
            field("run", offset_of!(PmpxCommandCap, run)),
            field("free_command", offset_of!(PmpxCommandCap, free_command)),
        ],
    },
    Spec {
        name: "PmpxAttach",
        size: size_of::<PmpxAttach>(),
        align: align_of::<PmpxAttach>(),
        fields: &[
            field("size", offset_of!(PmpxAttach, size)),
            field("attach", offset_of!(PmpxAttach, attach)),
        ],
    },
    Spec {
        name: "PmpxLog",
        size: size_of::<PmpxLog>(),
        align: align_of::<PmpxLog>(),
        fields: &[
            field("size", offset_of!(PmpxLog, size)),
            field("write", offset_of!(PmpxLog, write)),
            field("max_level", offset_of!(PmpxLog, max_level)),
        ],
    },
];

/// The numbers that go with the data, as `(snapshot key, macro name in the C header, value)`.
///
/// The macro names are written out rather than derived: a Rust constant is `PMPX_OK` while a
/// mechanically produced name would be `PMPX_ERROR_OK`, and the header is the one place a C author
/// reads them.
const NUMBERS: &[(&str, &str, u32)] = &[
    ("verb.install", "PMPX_VERB_INSTALL", PMPX_VERB_INSTALL),
    ("verb.remove", "PMPX_VERB_REMOVE", PMPX_VERB_REMOVE),
    ("verb.update", "PMPX_VERB_UPDATE", PMPX_VERB_UPDATE),
    ("verb.run", "PMPX_VERB_RUN", PMPX_VERB_RUN),
    ("verb.build", "PMPX_VERB_BUILD", PMPX_VERB_BUILD),
    ("verb.test", "PMPX_VERB_TEST", PMPX_VERB_TEST),
    ("verb.exec", "PMPX_VERB_EXEC", PMPX_VERB_EXEC),
    ("reason.scored", "PMPX_REASON_SCORED", PMPX_REASON_SCORED),
    ("reason.pinned", "PMPX_REASON_PINNED", PMPX_REASON_PINNED),
    (
        "reason.explicit",
        "PMPX_REASON_EXPLICIT",
        PMPX_REASON_EXPLICIT,
    ),
    ("error.ok", "PMPX_OK", PMPX_OK),
    (
        "error.unsupported_verb",
        "PMPX_ERR_UNSUPPORTED_VERB",
        PMPX_ERR_UNSUPPORTED_VERB,
    ),
    (
        "error.invalid_args",
        "PMPX_ERR_INVALID_ARGS",
        PMPX_ERR_INVALID_ARGS,
    ),
    ("error.internal", "PMPX_ERR_INTERNAL", PMPX_ERR_INTERNAL),
    ("level.error", "PMPX_LEVEL_ERROR", PMPX_LEVEL_ERROR),
    ("level.warn", "PMPX_LEVEL_WARN", PMPX_LEVEL_WARN),
    ("level.info", "PMPX_LEVEL_INFO", PMPX_LEVEL_INFO),
    ("level.debug", "PMPX_LEVEL_DEBUG", PMPX_LEVEL_DEBUG),
];

/// Write `PMPX_<NAME>` for a dotted name, with dots turned into underscores and ASCII letters
/// upper-cased -- without allocating, because this crate is `no_std`.
fn write_macro_name(out: &mut impl Write, prefix: &str, name: &str) -> fmt::Result {
    write!(out, "{prefix}")?;
    for ch in name.chars() {
        if ch == '.' {
            write!(out, "_")?;
        } else {
            write!(out, "{}", ch.to_ascii_uppercase())?;
        }
    }
    Ok(())
}

/// The name one struct has in the C header.
fn c_struct_name(rust_name: &str) -> &str {
    match rust_name {
        // `PmpxSlice<PmpxStr>` is `PmpxStrSlice` in C.
        "PmpxSlice<PmpxStr>" => "PmpxStrSlice",
        other => other,
    }
}

/// Write one `name = [ ... ]` list of strings, one entry per line so a diff is readable.
///
/// Takes an iterator rather than a slice: this crate is `no_std`, so the renderers collect nothing.
fn write_list<'a>(
    out: &mut impl Write,
    name: &str,
    items: impl Iterator<Item = &'a str>,
) -> fmt::Result {
    writeln!(out, "{name} = [")?;
    for item in items {
        writeln!(out, "  \"{item}\",")?;
    }
    writeln!(out, "]")
}

/// Render the snapshot: the ABI's surface, in a form a diff can be read from.
pub fn write_snapshot(out: &mut impl Write) -> fmt::Result {
    writeln!(
        out,
        "# Generated by `cargo run -p pmpx-plugin-abi --example gen-abi-files`."
    )?;
    writeln!(
        out,
        "# Do not edit: a test compares this file with the definitions."
    )?;
    writeln!(out, "abi_major = {PMPX_ABI_MAJOR}")?;
    writeln!(out, "max_items = {PMPX_MAX_ITEMS}")?;
    writeln!(out, "pointer_bytes = {}", size_of::<usize>())?;
    writeln!(out)?;

    write_list(out, "keys", PMPX_KEYS.iter().map(|(key, _)| *key))?;
    write_list(out, "caps", PMPX_CAPS.iter().copied())?;
    write_list(out, "required_caps", PMPX_REQUIRED_CAPS.iter().copied())?;
    writeln!(out)?;

    for (name, _macro_name, value) in NUMBERS {
        // `verb.install = 0`, so one number per line: an inserted verb moves nothing.
        writeln!(out, "{name} = {value}")?;
    }
    writeln!(out)?;

    for spec in SPECS {
        writeln!(out, "[[struct]]")?;
        writeln!(out, "name = \"{}\"", spec.name)?;
        writeln!(out, "size = {}", spec.size)?;
        writeln!(out, "align = {}", spec.align)?;

        writeln!(out, "fields = [")?;
        for f in spec.fields {
            writeln!(out, "  \"{}:{}\",", f.name, f.offset)?;
        }
        writeln!(out, "]")?;
    }

    Ok(())
}

/// Render the C header, with the layout this module computed as `_Static_assert`s.
pub fn write_c_header(out: &mut impl Write) -> fmt::Result {
    writeln!(
        out,
        "/* Generated by `cargo run -p pmpx-plugin-abi --example gen-abi-files`. Do not edit. */"
    )?;
    writeln!(
        out,
        "/* The pmpx plugin ABI, for a plugin written in C (or Zig, or Go...). */"
    )?;
    writeln!(out, "#ifndef PMPX_PLUGIN_H")?;
    writeln!(out, "#define PMPX_PLUGIN_H")?;
    writeln!(out)?;
    writeln!(out, "#include <stddef.h>")?;
    writeln!(out, "#include <stdint.h>")?;
    writeln!(out)?;
    writeln!(out, "#ifdef __cplusplus")?;
    writeln!(out, "extern \"C\" {{")?;
    writeln!(out, "#endif")?;
    writeln!(out)?;
    writeln!(out, "#define PMPX_ABI_MAJOR {PMPX_ABI_MAJOR}u")?;
    writeln!(out, "#define PMPX_MAX_ITEMS {PMPX_MAX_ITEMS}u")?;
    writeln!(out)?;

    writeln!(out, "/* ---- The numbers ---- */")?;
    for (_name, macro_name, value) in NUMBERS {
        writeln!(out, "#define {macro_name} {value}u")?;
    }
    writeln!(out)?;

    writeln!(out, "/* ---- Context keys ---- */")?;
    for (key, macro_name) in PMPX_KEYS {
        writeln!(out, "#define {macro_name} \"{key}\"")?;
    }
    writeln!(out)?;

    writeln!(out, "/* ---- Capabilities ---- */")?;
    for cap in PMPX_CAPS {
        write!(out, "#define ")?;
        write_macro_name(out, "PMPX_CAP_", cap)?;
        writeln!(out, " \"{cap}\"")?;
    }
    writeln!(out)?;

    writeln!(out, "/* ---- The data ---- */")?;
    writeln!(out, "typedef struct PmpxStr {{")?;
    writeln!(
        out,
        "    const uint8_t *ptr; /* null means the value is absent */"
    )?;
    writeln!(out, "    size_t len;")?;
    writeln!(out, "}} PmpxStr;")?;
    writeln!(out)?;
    writeln!(out, "typedef struct PmpxStrSlice {{")?;
    writeln!(out, "    const PmpxStr *ptr;")?;
    writeln!(out, "    size_t len;")?;
    writeln!(out, "}} PmpxStrSlice;")?;
    writeln!(out)?;

    writeln!(out, "typedef struct PmpxContext PmpxContext;")?;
    writeln!(out, "typedef struct PmpxHost PmpxHost;")?;
    writeln!(out)?;

    writeln!(out, "struct PmpxContext {{")?;
    writeln!(out, "    size_t size;")?;
    writeln!(out, "    uint32_t verb;")?;
    writeln!(out, "    uint32_t reason;")?;
    writeln!(out, "    uint32_t score;")?;
    writeln!(
        out,
        "    size_t (*count)(const PmpxContext *context, PmpxStr key);"
    )?;
    writeln!(
        out,
        "    PmpxStr (*get)(const PmpxContext *context, PmpxStr key, size_t index);"
    )?;
    writeln!(
        out,
        "    PmpxStr (*name)(const PmpxContext *context, PmpxStr key, size_t index);"
    )?;
    writeln!(out, "}};")?;
    writeln!(out)?;

    writeln!(out, "struct PmpxHost {{")?;
    writeln!(out, "    uint32_t abi_major;")?;
    writeln!(out, "    size_t size;")?;
    writeln!(out, "    const void *(*capability)(PmpxStr name);")?;
    writeln!(out, "}};")?;
    writeln!(out)?;

    writeln!(out, "typedef struct PmpxPlugin {{")?;
    writeln!(out, "    uint32_t abi_major;")?;
    writeln!(out, "    PmpxStr rustc_version;")?;
    writeln!(out, "    PmpxStr target;")?;
    writeln!(out, "    const void *(*capability)(PmpxStr name);")?;
    writeln!(out, "}} PmpxPlugin;")?;
    writeln!(out)?;

    writeln!(out, "typedef struct PmpxCommand {{")?;
    writeln!(out, "    size_t size;")?;
    writeln!(out, "    PmpxStr program;")?;
    writeln!(out, "    PmpxStrSlice args;")?;
    writeln!(out, "    PmpxStr cwd; /* absent means the project root */")?;
    writeln!(out, "}} PmpxCommand;")?;
    writeln!(out)?;

    writeln!(out, "typedef struct PmpxIdentity {{")?;
    writeln!(out, "    size_t size;")?;
    writeln!(out, "    PmpxStr (*name)(void);")?;
    writeln!(out, "    PmpxStr (*family)(void);")?;
    writeln!(out, "    void (*free_str)(PmpxStr s);")?;
    writeln!(out, "}} PmpxIdentity;")?;
    writeln!(out)?;

    writeln!(out, "typedef struct PmpxCommandCap {{")?;
    writeln!(out, "    size_t size;")?;
    writeln!(
        out,
        "    uint32_t (*run)(const PmpxContext *context, PmpxCommand *out);"
    )?;
    writeln!(out, "    void (*free_command)(PmpxCommand *command);")?;
    writeln!(out, "}} PmpxCommandCap;")?;
    writeln!(out)?;

    writeln!(out, "typedef struct PmpxAttach {{")?;
    writeln!(out, "    size_t size;")?;
    writeln!(out, "    void (*attach)(const PmpxHost *host);")?;
    writeln!(out, "}} PmpxAttach;")?;
    writeln!(out)?;

    writeln!(out, "typedef struct PmpxLog {{")?;
    writeln!(out, "    size_t size;")?;
    writeln!(out, "    void (*write)(uint32_t level, PmpxStr message);")?;
    writeln!(out, "    uint32_t max_level;")?;
    writeln!(out, "}} PmpxLog;")?;
    writeln!(out)?;

    writeln!(out, "/* ---- Layout, checked by the C compiler ---- */")?;
    writeln!(
        out,
        "#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L"
    )?;
    for spec in SPECS {
        // `PmpxSlice<PmpxStr>` is `PmpxStrSlice` in C.
        let c_name = c_struct_name(spec.name);
        writeln!(
            out,
            "_Static_assert(sizeof({c_name}) == {}, \"{c_name} size\");",
            spec.size
        )?;
        for f in spec.fields {
            writeln!(
                out,
                "_Static_assert(offsetof({c_name}, {}) == {}, \"{c_name}.{}\");",
                f.name, f.offset, f.name
            )?;
        }
    }
    writeln!(out, "#endif")?;
    writeln!(out)?;

    writeln!(out, "#ifdef __cplusplus")?;
    writeln!(out, "}}")?;
    writeln!(out, "#endif")?;
    writeln!(out)?;
    writeln!(out, "#endif /* PMPX_PLUGIN_H */")
}

#[cfg(test)]
mod tests {
    // The library is `no_std`; the test harness is not, and needs `String` to compare with the
    // committed files.
    extern crate std;

    use std::string::String;
    use std::vec::Vec;

    use super::*;

    fn rendered(render: impl FnOnce(&mut String) -> fmt::Result) -> String {
        let mut out = String::new();
        render(&mut out).expect("writing into a String cannot fail");
        out
    }

    /// The macro name a key or capability gets in the header, built the same way the header builds
    /// it.
    fn macro_name(prefix: &str, name: &str) -> String {
        let mut out = String::new();
        write_macro_name(&mut out, prefix, name).expect("writing into a String cannot fail");
        out
    }

    /// The snapshot is the guard that keeps a wire-format change from happening quietly: any change
    /// to the definitions makes this fail until `abi/surface.toml` is regenerated **and reviewed**.
    #[test]
    fn the_snapshot_matches_the_definitions() {
        let want = include_str!("../abi/surface.toml");
        let got = rendered(write_snapshot);

        assert_eq!(
            got, want,
            "the ABI surface changed: regenerate with \
             `cargo run -p pmpx-plugin-abi --example gen-abi-files`, then check whether \
             PMPX_ABI_MAJOR has to move (docs/refactor.md §3.3)"
        );
    }

    /// The C header is generated from the same numbers, so a C plugin and a Rust plugin cannot be
    /// reading different layouts.
    #[test]
    fn the_c_header_matches_the_definitions() {
        let want = include_str!("../include/pmpx_plugin.h");
        let got = rendered(write_c_header);

        assert_eq!(
            got, want,
            "include/pmpx_plugin.h is out of date: regenerate with \
             `cargo run -p pmpx-plugin-abi --example gen-abi-files`"
        );
    }

    /// The snapshot is recorded for a 64-bit target; the supported platforms are all 64-bit, and
    /// saying so out loud beats a confusing diff on a 32-bit one.
    #[test]
    fn the_snapshot_was_recorded_for_this_pointer_width() {
        let want: usize = include_str!("../abi/surface.toml")
            .lines()
            .find_map(|line| line.strip_prefix("pointer_bytes = "))
            .and_then(|value| value.parse().ok())
            .expect("the snapshot records pointer_bytes");

        assert_eq!(
            want,
            size_of::<usize>(),
            "the frozen surface was recorded on a {want}-byte-pointer target"
        );
    }

    /// Every key and capability appears in both renderings: a name that exists but is unreachable
    /// from C would be a silent hole for a non-Rust plugin.
    #[test]
    fn every_name_reaches_the_header() {
        let header = rendered(write_c_header);

        for (key, macro_name) in PMPX_KEYS {
            assert!(
                header.contains(macro_name),
                "{macro_name} is missing for the key {key}"
            );
        }
        for cap in PMPX_CAPS {
            let name = macro_name("PMPX_CAP_", cap);
            assert!(header.contains(&name), "{name} is missing");
        }
    }

    #[test]
    fn the_required_capabilities_are_a_subset_of_the_known_ones() {
        for required in PMPX_REQUIRED_CAPS {
            assert!(
                PMPX_CAPS.contains(required),
                "{required} is not a known capability"
            );
        }
    }

    /// The names themselves are part of the ABI: a typo here would be a key nobody answers.
    #[test]
    fn the_lists_are_sorted_and_unique() {
        let mut lists: [Vec<&str>; 3] = [
            PMPX_KEYS.iter().map(|(key, _)| *key).collect(),
            PMPX_KEYS
                .iter()
                .map(|(_, macro_name)| *macro_name)
                .collect(),
            PMPX_CAPS.to_vec(),
        ];

        for list in &mut lists {
            let before = list.len();
            list.sort_unstable();
            list.dedup();
            assert_eq!(list.len(), before, "a name is repeated in {list:?}");
        }
    }
}
