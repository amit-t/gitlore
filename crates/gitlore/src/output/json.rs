//! ADR-030 JSON envelope helpers (SPEC-001 §4.3).
//!
//! Every `--json` response is a single UTF-8 newline-terminated JSON object
//! with `"schema_version": 1` at the top level.
//!
//! Success:
//! ```json
//! {"schema_version":1,"data":{...}}
//! ```
//!
//! Error:
//! ```json
//! {"schema_version":1,"error":{"code":"...","message":"...","details":null}}
//! ```

use serde_json::{json, Value};

/// Schema version stamped on every envelope.
pub const SCHEMA_VERSION: u32 = 1;

/// Wrap a `serde_json::Value` in the success envelope and return it as a
/// single-line JSON string (no trailing newline — the caller adds one).
///
/// # Panics
/// Panics only if serialization fails, which should never happen for `Value`.
pub fn envelope(data: Value) -> String {
    let v = json!({
        "schema_version": SCHEMA_VERSION,
        "data": data,
    });
    serde_json::to_string(&v).expect("envelope serialization must not fail")
}

/// Build the error envelope from a stable code string and a human message.
///
/// Returns a single-line JSON string (no trailing newline).
pub fn error_envelope(code: &str, message: &str) -> String {
    let v = json!({
        "schema_version": SCHEMA_VERSION,
        "error": {
            "code": code,
            "message": message,
            "details": Value::Null,
        },
    });
    serde_json::to_string(&v).expect("error_envelope serialization must not fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn envelope_has_schema_version_and_data() {
        let raw = envelope(json!({"foo": 1}));
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["schema_version"], SCHEMA_VERSION as i64);
        assert_eq!(v["data"]["foo"], 1);
        assert!(v.get("error").is_none());
    }

    #[test]
    fn envelope_is_single_line() {
        let raw = envelope(json!({"key": "value"}));
        assert!(!raw.contains('\n'));
    }

    #[test]
    fn error_envelope_has_schema_version_code_message() {
        let raw = error_envelope("sha_ambiguous_prefix", "prefix 'dead' is ambiguous");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["schema_version"], SCHEMA_VERSION as i64);
        assert_eq!(v["error"]["code"], "sha_ambiguous_prefix");
        assert_eq!(v["error"]["message"], "prefix 'dead' is ambiguous");
        assert_eq!(v["error"]["details"], Value::Null);
        assert!(v.get("data").is_none());
    }

    #[test]
    fn error_envelope_is_single_line() {
        let raw = error_envelope("invalid_query", "empty query");
        assert!(!raw.contains('\n'));
    }

    #[test]
    fn envelope_with_array_data() {
        let raw = envelope(json!([1, 2, 3]));
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert!(v["data"].is_array());
    }
}
