use std::path::Path;

use mangodisk_core::diagnostic_path;
use serde_json::Value;

/// Redacts every absolute-path string in a serialized tool response in place.
///
/// Redaction is value-based rather than field-based: Core result types evolve,
/// and a missed field name would silently leak a private path. Treating any
/// absolute path as sensitive keeps the failure mode on the safe side — a
/// non-path string that looks absolute is over-redacted, never under-redacted.
pub(crate) fn redact_paths(value: &mut Value) {
    match value {
        Value::String(text) => {
            if is_absolute_path(text) {
                *text = diagnostic_path(Path::new(text));
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_paths(item);
            }
        }
        Value::Object(fields) => {
            for field in fields.values_mut() {
                redact_paths(field);
            }
        }
        _ => {}
    }
}

/// Recognizes Unix (`/…`), Windows drive (`C:\…`, `C:/…`), and UNC (`\\…`)
/// absolute paths without touching the filesystem, so redaction works for
/// foreign-path strings on any host platform.
fn is_absolute_path(value: &str) -> bool {
    if value.starts_with('/') || value.starts_with("\\\\") {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn absolute_paths_are_redacted_recursively() {
        let mut value = json!({
            "root": "/Users/alice/projects/demo",
            "entries": [
                { "path": "/Users/alice/projects/demo/target", "bytes": 42 },
                { "path": "/Users/alice/projects/demo/Cargo.toml", "note": "not a path" }
            ],
            "count": 2,
            "nested": { "deep": [{ "parentPath": "/Users/alice/projects/demo" }] }
        });

        redact_paths(&mut value);

        let text = value.to_string();
        assert!(!text.contains("/Users/alice"));
        assert!(!text.contains("projects/demo"));
        // The leaf name stays readable and carries a stable digest suffix.
        let root = value["root"].as_str().expect("redacted root");
        assert!(root.starts_with("demo#"), "unexpected redaction: {root}");
        assert_eq!(root.len(), "demo#".len() + 12);
        assert!(value["entries"][0]["path"]
            .as_str()
            .expect("redacted path")
            .starts_with("target#"));
        assert_eq!(value["entries"][1]["note"], "not a path");
        assert_eq!(value["count"], 2);
    }

    #[test]
    fn windows_style_paths_are_redacted_on_any_host() {
        let mut value = json!({ "path": "C:\\Users\\alice\\cache" });

        redact_paths(&mut value);

        assert!(!value.to_string().contains("C:\\Users"));
    }

    #[test]
    fn relative_and_plain_strings_are_untouched() {
        let mut value = json!({
            "ruleId": "development.npm-cache",
            "relative": "projects/demo",
            "hash": "0123456789abcdef"
        });

        redact_paths(&mut value);

        assert_eq!(value["ruleId"], "development.npm-cache");
        assert_eq!(value["relative"], "projects/demo");
        assert_eq!(value["hash"], "0123456789abcdef");
    }
}
