//! JSON-RPC 2.0 messages as LSP uses them: requests, responses and notifications.

use serde::Serialize;
use serde_json::Value;

use crate::finding::{Finding, Severity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Request,
    Response,
    Notification,
    Invalid,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Classified {
    pub kind: Kind,
    pub method: Option<String>,
    pub id: Option<Value>,
}

/// The key that pairs a request with its response.
pub fn id_key(id: &Value) -> String {
    id.to_string()
}

pub fn classify(value: &Value, offset: u64, findings: &mut Vec<Finding>) -> Classified {
    let mut error = |code: &'static str, message: String| {
        findings.push(Finding::new(Severity::Error, code, message, offset));
    };
    let invalid = Classified {
        kind: Kind::Invalid,
        method: None,
        id: None,
    };

    let Some(object) = value.as_object() else {
        error(
            "jsonrpc/not-an-object",
            "a message must be a JSON object".to_string(),
        );
        return invalid;
    };

    if object.get("jsonrpc") != Some(&Value::String("2.0".to_string())) {
        error(
            "jsonrpc/version",
            "a message must have \"jsonrpc\": \"2.0\"".to_string(),
        );
    }

    let id = object.get("id").cloned();

    if let Some(method) = object.get("method") {
        let Some(method) = method.as_str() else {
            error(
                "jsonrpc/method-not-a-string",
                "\"method\" must be a string".to_string(),
            );
            return invalid;
        };
        if let Some(params) = object.get("params")
            && !params.is_object()
            && !params.is_array()
        {
            error(
                "jsonrpc/params-structure",
                format!("\"params\" of {method} must be an object or an array"),
            );
        }
        let kind = match &id {
            None => Kind::Notification,
            Some(id) => {
                if !is_request_id(id) {
                    error(
                        "jsonrpc/request-id-type",
                        format!("the id of a request must be an integer or a string, not {id}"),
                    );
                }
                Kind::Request
            }
        };
        return Classified {
            kind,
            method: Some(method.to_string()),
            id,
        };
    }

    let Some(id) = id else {
        error(
            "jsonrpc/unknown-shape",
            "a message without \"method\" must be a response with an \"id\"".to_string(),
        );
        return invalid;
    };
    if !is_request_id(&id) && !id.is_null() {
        error(
            "jsonrpc/response-id-type",
            format!("the id of a response must be an integer, a string or null, not {id}"),
        );
    }
    match (object.get("result"), object.get("error")) {
        (Some(_), Some(_)) => error(
            "jsonrpc/result-and-error",
            "a response must not have both \"result\" and \"error\"".to_string(),
        ),
        (None, None) => error(
            "jsonrpc/response-without-result",
            "a response must have \"result\" or \"error\"; a successful response without a value has \"result\": null".to_string(),
        ),
        (None, Some(err)) => check_error_object(err, &mut error),
        (Some(_), None) => {}
    }
    Classified {
        kind: Kind::Response,
        method: None,
        id: Some(id),
    }
}

fn is_request_id(id: &Value) -> bool {
    id.is_string() || id.as_i64().is_some() || id.as_u64().is_some()
}

fn check_error_object(err: &Value, error: &mut impl FnMut(&'static str, String)) {
    let Some(object) = err.as_object() else {
        error(
            "jsonrpc/error-not-an-object",
            "\"error\" must be an object".to_string(),
        );
        return;
    };
    if object.get("code").and_then(Value::as_i64).is_none() {
        error(
            "jsonrpc/error-code",
            "\"error.code\" must be an integer".to_string(),
        );
    }
    if !object.get("message").is_some_and(Value::is_string) {
        error(
            "jsonrpc/error-message",
            "\"error.message\" must be a string".to_string(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(value: Value) -> (Classified, Vec<&'static str>) {
        let mut findings = Vec::new();
        let classified = classify(&value, 0, &mut findings);
        (classified, findings.iter().map(|f| f.code).collect())
    }

    #[test]
    fn classifies_requests_notifications_and_responses() {
        let (c, f) = run(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
        assert_eq!(c.kind, Kind::Request);
        assert_eq!(c.method.as_deref(), Some("initialize"));
        assert!(f.is_empty());

        let (c, f) = run(json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
        assert_eq!(c.kind, Kind::Notification);
        assert!(f.is_empty());

        let (c, f) = run(json!({"jsonrpc": "2.0", "id": "a", "result": null}));
        assert_eq!(c.kind, Kind::Response);
        assert!(f.is_empty());
    }

    #[test]
    fn reports_malformed_messages() {
        assert_eq!(run(json!([1])).1, vec!["jsonrpc/not-an-object"]);
        assert_eq!(
            run(json!({"id": 1, "method": "x"})).1,
            vec!["jsonrpc/version"]
        );
        assert_eq!(
            run(json!({"jsonrpc": "2.0", "id": 1.5, "method": "x"})).1,
            vec!["jsonrpc/request-id-type"]
        );
        assert_eq!(
            run(json!({"jsonrpc": "2.0", "method": "x", "params": 3})).1,
            vec!["jsonrpc/params-structure"]
        );
        assert_eq!(
            run(json!({"jsonrpc": "2.0", "id": 1})).1,
            vec!["jsonrpc/response-without-result"]
        );
        assert_eq!(
            run(json!({"jsonrpc": "2.0", "id": 1, "result": 1, "error": {}})).1,
            vec!["jsonrpc/result-and-error"]
        );
        assert_eq!(
            run(json!({"jsonrpc": "2.0", "id": 1, "error": {"code": "x"}})).1,
            vec!["jsonrpc/error-code", "jsonrpc/error-message"]
        );
        assert_eq!(
            run(json!({"jsonrpc": "2.0", "result": 1})).1,
            vec!["jsonrpc/unknown-shape"]
        );
    }
}
