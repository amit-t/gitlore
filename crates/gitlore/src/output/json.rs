//! JSON envelope helpers per ADR-030 / SPEC-001 §4.3.
//!
//! Provides typed functions for emitting the standardized JSON output format
//! used by all `gitlore --json` subcommands:
//!
//! * Success envelopes wrap the payload in `{"schema_version":1,"data":...}`
//! * Error envelopes use `{"schema_version":1,"error":{"code":...,"message":...,"details":null}}`
//!
//! All envelopes are emitted as single-line JSON (no pretty-printing) so
//! scripted consumers can pipe directly into `jq` without whitespace handling.

use serde::Serialize;
use serde_json::Value;

/// Current schema version for the JSON envelope format.
///
/// This version is incremented when the envelope structure changes in a
/// backward-incompatible way. Consumers should check this field to ensure
/// they can parse the response correctly.
const SCHEMA_VERSION: u8 = 1;

/// Wrap a success payload in the standard JSON envelope.
///
/// Produces single-line JSON: `{"schema_version":1,"data":<payload>}`
///
/// # Type parameters
///
/// * `T` - Any type that implements `Serialize` (typically a struct or enum
///   representing the command's output)
///
/// # Example
///
/// ```ignore
/// use gitlore::output::json::envelope;
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct IndexReport {
///     commits_indexed: u64,
///     duration_ms: u64,
/// }
///
/// let report = IndexReport { commits_indexed: 100, duration_ms: 500 };
/// let json = envelope(&report);
/// // Output: {"schema_version":1,"data":{"commits_indexed":100,"duration_ms":500}}
/// ```
pub fn envelope<T: Serialize>(data: &T) -> String {
    serde_json::to_string(&serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "data": data,
    }))
    .expect("JSON serialization should never fail for the envelope structure")
}

/// Create a standard error envelope.
///
/// Produces single-line JSON: `{"schema_version":1,"error":{"code":"...","message":"...","details":null}}`
///
/// # Arguments
///
/// * `code` - Machine-readable error code (e.g., "not_a_repo", "lock_contention")
/// * `message` - Human-readable error message
///
/// # Example
///
/// ```ignore
/// use gitlore::output::json::error_envelope;
///
/// let json = error_envelope("not_a_repo", "not a git repository");
/// // Output: {"schema_version":1,"error":{"code":"not_a_repo","message":"not a git repository","details":null}}
/// ```
pub fn error_envelope(code: &str, message: &str) -> String {
    serde_json::to_string(&serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "error": {
            "code": code,
            "message": message,
            "details": Value::Null,
        }
    }))
    .expect("JSON serialization should never fail for the error envelope structure")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct TestPayload {
        value: String,
        count: u32,
    }

    #[test]
    fn envelope_includes_schema_version() {
        let payload = TestPayload {
            value: "test".to_string(),
            count: 42,
        };
        let json = envelope(&payload);
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["schema_version"], 1);
    }

    #[test]
    fn envelope_wraps_data_field() {
        let payload = TestPayload {
            value: "test".to_string(),
            count: 42,
        };
        let json = envelope(&payload);
        let v: Value = serde_json::from_str(&json).unwrap();
        assert!(v["data"].is_object());
        assert_eq!(v["data"]["value"], "test");
        assert_eq!(v["data"]["count"], 42);
    }

    #[test]
    fn envelope_is_single_line() {
        let payload = TestPayload {
            value: "test".to_string(),
            count: 42,
        };
        let json = envelope(&payload);
        assert!(!json.contains('\n'));
        assert!(!json.contains('\r'));
    }

    #[test]
    fn error_envelope_includes_schema_version() {
        let json = error_envelope("test_code", "test message");
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["schema_version"], 1);
    }

    #[test]
    fn error_envelope_has_error_structure() {
        let json = error_envelope("test_code", "test message");
        let v: Value = serde_json::from_str(&json).unwrap();
        assert!(v["error"].is_object());
        assert_eq!(v["error"]["code"], "test_code");
        assert_eq!(v["error"]["message"], "test message");
        assert_eq!(v["error"]["details"], Value::Null);
    }

    #[test]
    fn error_envelope_is_single_line() {
        let json = error_envelope("test_code", "test message");
        assert!(!json.contains('\n'));
        assert!(!json.contains('\r'));
    }
}
