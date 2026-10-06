//! `config get / set`: the global config file.
//!
//! Dotted keys (`plugin.family_priority`) and the read-modify-write that keeps unrecognised
//! keys are the whole of it. Writing touches the global config only -- a project
//! `.pmpx.toml` is written by `plugin set / unset`, not from here.

use std::path::Path;

use anyhow::{Context as _, Result};

use crate::cli::ConfigCommand;
use crate::error::{PmpxError, EXIT_OK};
use crate::style;

pub(super) fn config_cmd(cmd: &ConfigCommand) -> crate::error::Result<u8> {
    let path = crate::config::global_config_path().map_err(PmpxError::Other)?;

    match cmd {
        ConfigCommand::Get { key } => {
            let doc = read_toml_table(&path).map_err(PmpxError::Other)?;
            match lookup_dotted(&doc, key) {
                Some(v) => {
                    anstream::println!("{}", render_value(v));
                    Ok(EXIT_OK)
                }
                None => Err(PmpxError::Usage(format!(
                    "{} has no {key} (config file: {})",
                    "global config",
                    path.display()
                ))),
            }
        }

        ConfigCommand::Set { key, value } => {
            let mut doc = read_toml_table(&path).map_err(PmpxError::Other)?;
            // Parse once: what gets printed has to be the value that was stored, and a second
            // independent parse is one more chance for the two to differ.
            let parsed = parse_value(value);
            insert_dotted(&mut doc, key, parsed.clone())
                .map_err(|e| PmpxError::Usage(e.to_string()))?;

            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| PmpxError::Other(e.into()))?;
            }
            let text = toml::to_string_pretty(&doc).map_err(|e| PmpxError::Other(e.into()))?;

            // Checked *before* anything is written, because the reader is strict about types: without
            // this, `config set discovery.walk_up maybe` would be accepted happily, and then every
            // later command would fail to read the file at all -- a typo turning into an unusable
            // installation.
            if let Err(error) = toml::from_str::<crate::config::GlobalConfig>(&text) {
                return Err(PmpxError::Usage(format!(
                    "{key} = {} is not a valid value: {error}\nNothing was written.",
                    render_value(&parsed)
                )));
            }

            crate::config::atomic_write(&path, &text).map_err(PmpxError::Other)?;

            anstream::println!(
                "Wrote {key} = {} to {}",
                render_value(&parsed),
                style::paint(style::DIM, path.display())
            );
            Ok(EXIT_OK)
        }
    }
}

/// Read a TOML file into a table; a missing file = an empty table.
pub(super) fn read_toml_table(path: &Path) -> Result<toml::Table> {
    match std::fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => {
            toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
        }
        Ok(_) => Ok(toml::Table::new()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Look up a value by `a.b.c`.
pub(super) fn lookup_dotted<'a>(table: &'a toml::Table, key: &str) -> Option<&'a toml::Value> {
    let mut parts = key.split('.');
    let first = parts.next()?;
    let mut current = table.get(first)?;

    for part in parts {
        current = current.as_table()?.get(part)?;
    }
    Some(current)
}

/// Write a value by `a.b.c`; missing intermediate tables are created.
pub(super) fn insert_dotted(table: &mut toml::Table, key: &str, value: toml::Value) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.iter().any(|p| p.is_empty()) {
        anyhow::bail!("a key must not have empty segments: {key}");
    }

    let mut current = table;
    for part in &parts[..parts.len() - 1] {
        let entry = current
            .entry((*part).to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));

        current = entry
            .as_table_mut()
            .with_context(|| format!("{part} is not a table, cannot write into it"))?;
    }

    current.insert(parts[parts.len() - 1].to_string(), value);
    Ok(())
}

/// Parse a string from the command line into a TOML value.
///
/// The order is deliberately "most specific to most permissive": `true` -> integer -> array
/// -> string. So `pmpx config set x 123` stores a number; to store a string, write `"123"`
/// (with quotes).
pub(super) fn parse_value(raw: &str) -> toml::Value {
    if raw == "true" {
        return toml::Value::Boolean(true);
    }
    if raw == "false" {
        return toml::Value::Boolean(false);
    }
    if let Ok(n) = raw.parse::<i64>() {
        return toml::Value::Integer(n);
    }

    // Arrays and quoted strings borrow TOML's own parser
    if let Ok(doc) = toml::from_str::<toml::Table>(&format!("v = {raw}")) {
        if let Some(v) = doc.get("v") {
            return v.clone();
        }
    }

    toml::Value::String(raw.to_string())
}

/// Print a TOML value. Strings get no quotes -- the user wants the value, not the syntax.
pub(super) fn render_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string().trim().to_string(),
    }
}

// self

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_value_recognises_booleans() {
        assert_eq!(parse_value("true"), toml::Value::Boolean(true));
        assert_eq!(parse_value("false"), toml::Value::Boolean(false));
    }

    #[test]
    fn parse_value_recognises_integers() {
        assert_eq!(parse_value("42"), toml::Value::Integer(42));
        assert_eq!(parse_value("-7"), toml::Value::Integer(-7));
    }

    #[test]
    fn parse_value_recognises_arrays() {
        let v = parse_value("[\"rust\", \"node\"]");
        let arr = v.as_array().expect("should be an array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str(), Some("rust"));
    }

    #[test]
    fn parse_value_falls_back_to_a_string() {
        assert_eq!(parse_value("node"), toml::Value::String("node".to_string()));
        // A number stored as a string has to carry quotes
        assert_eq!(
            parse_value("\"123\""),
            toml::Value::String("123".to_string())
        );
    }

    #[test]
    fn render_value_omits_quotes_for_strings() {
        assert_eq!(render_value(&toml::Value::String("node".into())), "node");
        assert_eq!(render_value(&toml::Value::Integer(3)), "3");
        assert_eq!(render_value(&toml::Value::Boolean(true)), "true");
    }

    #[test]
    fn lookup_dotted_walks_tables() {
        let doc: toml::Table = toml::from_str(
            r#"
[plugin]
family_priority = ["node"]
[deep]
[deep.er]
x = 1
"#,
        )
        .unwrap();

        assert!(lookup_dotted(&doc, "plugin.family_priority").is_some());
        assert_eq!(
            lookup_dotted(&doc, "deep.er.x"),
            Some(&toml::Value::Integer(1))
        );
        assert!(lookup_dotted(&doc, "plugin.nope").is_none());
        assert!(lookup_dotted(&doc, "nope.at.all").is_none());
    }

    #[test]
    fn insert_dotted_creates_missing_tables() {
        let mut doc = toml::Table::new();
        insert_dotted(&mut doc, "a.b.c", toml::Value::Integer(1)).unwrap();

        assert_eq!(lookup_dotted(&doc, "a.b.c"), Some(&toml::Value::Integer(1)));
    }

    #[test]
    fn insert_dotted_refuses_to_clobber_a_non_table() {
        let mut doc: toml::Table = toml::from_str("a = 1\n").unwrap();
        let err = insert_dotted(&mut doc, "a.b", toml::Value::Integer(2)).unwrap_err();
        assert!(err.to_string().contains("not a table"), "{err}");
    }

    #[test]
    fn insert_dotted_rejects_empty_segments() {
        let mut doc = toml::Table::new();
        assert!(insert_dotted(&mut doc, "a..b", toml::Value::Integer(1)).is_err());
        assert!(insert_dotted(&mut doc, "", toml::Value::Integer(1)).is_err());
    }

    #[test]
    fn read_toml_table_treats_a_missing_file_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let t = read_toml_table(&tmp.path().join("nope.toml")).unwrap();
        assert!(t.is_empty());
    }

    #[test]
    fn read_toml_table_treats_an_empty_file_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("empty.toml");
        std::fs::write(&p, "   \n").unwrap();
        assert!(read_toml_table(&p).unwrap().is_empty());
    }
}
