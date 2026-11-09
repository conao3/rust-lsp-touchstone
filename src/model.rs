//! The LSP specification's meta model (`spec/metaModel.json`), embedded and interpreted: message
//! definitions, capability bindings and a validator for every type the specification defines.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

use crate::finding::Severity;

const META_MODEL: &str = include_str!("../spec/metaModel.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageDirection {
    ClientToServer,
    ServerToClient,
    Both,
}

#[derive(Debug, Clone)]
pub struct MessageDef {
    pub method: String,
    pub is_request: bool,
    pub direction: MessageDirection,
    pub params: Option<Value>,
    pub result: Option<Value>,
    pub client_capability: Option<String>,
    pub server_capability: Option<String>,
    pub registration_method: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

pub struct Model {
    pub version: String,
    messages: HashMap<String, MessageDef>,
    structures: HashMap<String, Value>,
    enumerations: HashMap<String, Value>,
    aliases: HashMap<String, Value>,
}

pub fn model() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| {
        Model::parse(META_MODEL).expect("spec/metaModel.json is a valid meta model")
    })
}

fn by_name(value: &Value, key: &str) -> Result<HashMap<String, Value>, String> {
    value[key]
        .as_array()
        .ok_or_else(|| format!("the meta model has no {key}"))?
        .iter()
        .map(|item| {
            let name = item["name"]
                .as_str()
                .ok_or_else(|| format!("an entry of {key} has no name"))?;
            Ok((name.to_string(), item.clone()))
        })
        .collect()
}

impl Model {
    fn parse(text: &str) -> Result<Model, String> {
        let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let mut messages = HashMap::new();
        for (key, is_request) in [("requests", true), ("notifications", false)] {
            for def in value[key]
                .as_array()
                .ok_or("the meta model has no messages")?
            {
                let method = def["method"].as_str().ok_or("a message has no method")?;
                let direction = match def["messageDirection"].as_str() {
                    Some("clientToServer") => MessageDirection::ClientToServer,
                    Some("serverToClient") => MessageDirection::ServerToClient,
                    Some("both") => MessageDirection::Both,
                    other => return Err(format!("{method} has the direction {other:?}")),
                };
                let text = |key: &str| def[key].as_str().map(str::to_string);
                messages.insert(
                    method.to_string(),
                    MessageDef {
                        method: method.to_string(),
                        is_request,
                        direction,
                        params: def.get("params").cloned(),
                        result: def.get("result").cloned(),
                        client_capability: text("clientCapability"),
                        server_capability: text("serverCapability"),
                        registration_method: text("registrationMethod"),
                    },
                );
            }
        }
        Ok(Model {
            version: value["metaData"]["version"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            messages,
            structures: by_name(&value, "structures")?,
            enumerations: by_name(&value, "enumerations")?,
            aliases: by_name(&value, "typeAliases")?,
        })
    }

    pub fn message(&self, method: &str) -> Option<&MessageDef> {
        self.messages.get(method)
    }

    /// The definitions whose dynamic registration uses `method`.
    pub fn registered_by(&self, method: &str) -> impl Iterator<Item = &MessageDef> {
        self.messages
            .values()
            .filter(move |def| def.registration_method.as_deref().unwrap_or(&def.method) == method)
    }

    /// Validates `value` against a type of the meta model; issues carry JSON pointer paths.
    pub fn validate(&self, ty: &Value, value: &Value, path: &str) -> Vec<Issue> {
        let mut issues = Vec::new();
        self.check(ty, value, path, &mut issues);
        issues
    }

    fn check(&self, ty: &Value, value: &Value, path: &str, issues: &mut Vec<Issue>) {
        let mut error = |code: &'static str, message: String| {
            issues.push(Issue {
                severity: Severity::Error,
                code,
                path: path.to_string(),
                message,
            });
        };
        match ty["kind"].as_str().unwrap_or_default() {
            "base" => {
                let name = ty["name"].as_str().unwrap_or_default();
                if let Err(expected) = check_base(name, value) {
                    error(
                        "types/wrong-type",
                        format!("expected {expected}, got {}", describe(value)),
                    );
                }
            }
            "stringLiteral" => {
                if value != &ty["value"] {
                    error(
                        "types/wrong-literal",
                        format!("expected {}, got {}", ty["value"], describe(value)),
                    );
                }
            }
            "array" => match value.as_array() {
                None => error(
                    "types/wrong-type",
                    format!("expected an array, got {}", describe(value)),
                ),
                Some(items) => {
                    for (i, item) in items.iter().enumerate() {
                        self.check(&ty["element"], item, &format!("{path}/{i}"), issues);
                    }
                }
            },
            "tuple" => {
                let expected = ty["items"].as_array().map_or(0, Vec::len);
                match value.as_array() {
                    Some(items) if items.len() == expected => {
                        for (i, (item, item_ty)) in items
                            .iter()
                            .zip(ty["items"].as_array().unwrap())
                            .enumerate()
                        {
                            self.check(item_ty, item, &format!("{path}/{i}"), issues);
                        }
                    }
                    _ => error(
                        "types/wrong-type",
                        format!("expected a tuple of {expected}, got {}", describe(value)),
                    ),
                }
            }
            "map" => match value.as_object() {
                None => error(
                    "types/wrong-type",
                    format!("expected an object, got {}", describe(value)),
                ),
                Some(object) => {
                    for (key, item) in object {
                        self.check(
                            &ty["value"],
                            item,
                            &format!("{path}/{}", escape(key)),
                            issues,
                        );
                    }
                }
            },
            "literal" => {
                let properties = ty["value"]["properties"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                self.check_object(&properties, value, path, issues);
            }
            "reference" => {
                let name = ty["name"].as_str().unwrap_or_default();
                if let Some(structure) = self.structures.get(name) {
                    let properties = self.properties(structure);
                    self.check_object(&properties, value, path, issues);
                } else if let Some(enumeration) = self.enumerations.get(name) {
                    self.check_enumeration(name, enumeration, value, path, issues);
                } else if let Some(alias) = self.aliases.get(name) {
                    self.check(&alias["type"], value, path, issues);
                } else {
                    error(
                        "types/unknown-type",
                        format!("the meta model has no type {name}"),
                    );
                }
            }
            "or" => self.check_or(ty, value, path, issues),
            "and" => {
                for item in ty["items"].as_array().into_iter().flatten() {
                    self.check(item, value, path, issues);
                }
            }
            other => error(
                "types/unknown-type",
                format!("the meta model has no type kind {other:?}"),
            ),
        }
    }

    fn check_or(&self, ty: &Value, value: &Value, path: &str, issues: &mut Vec<Issue>) {
        let alternatives: Vec<&Value> = ty["items"].as_array().into_iter().flatten().collect();
        let mut candidates = Vec::new();
        for alternative in &alternatives {
            let found = self.validate(alternative, value, path);
            let errors = found
                .iter()
                .filter(|i| i.severity == Severity::Error)
                .count();
            if errors == 0 {
                issues.extend(found);
                return;
            }
            if value.is_object() && self.is_object_type(alternative) {
                candidates.push(found);
            }
        }
        if candidates.len() == 1 {
            issues.extend(candidates.pop().unwrap());
            return;
        }
        issues.push(Issue {
            severity: Severity::Error,
            code: "types/no-alternative-matches",
            path: path.to_string(),
            message: format!(
                "{} matches none of {}",
                describe(value),
                alternatives
                    .iter()
                    .map(|a| type_name(a))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
        });
    }

    fn is_object_type(&self, ty: &Value) -> bool {
        match ty["kind"].as_str() {
            Some("literal") | Some("map") => true,
            Some("reference") => {
                let name = ty["name"].as_str().unwrap_or_default();
                self.structures.contains_key(name)
                    || self
                        .aliases
                        .get(name)
                        .is_some_and(|alias| self.is_object_type(&alias["type"]))
            }
            _ => false,
        }
    }

    fn properties(&self, structure: &Value) -> Vec<Value> {
        let mut properties = Vec::new();
        for key in ["extends", "mixins"] {
            for parent in structure[key].as_array().into_iter().flatten() {
                if let Some(parent) = parent["name"].as_str().and_then(|n| self.structures.get(n)) {
                    properties.extend(self.properties(parent));
                }
            }
        }
        for property in structure["properties"].as_array().into_iter().flatten() {
            properties.retain(|p: &Value| p["name"] != property["name"]);
            properties.push(property.clone());
        }
        properties
    }

    fn check_object(
        &self,
        properties: &[Value],
        value: &Value,
        path: &str,
        issues: &mut Vec<Issue>,
    ) {
        let Some(object) = value.as_object() else {
            issues.push(Issue {
                severity: Severity::Error,
                code: "types/wrong-type",
                path: path.to_string(),
                message: format!("expected an object, got {}", describe(value)),
            });
            return;
        };
        for property in properties {
            let name = property["name"].as_str().unwrap_or_default();
            let optional = property["optional"].as_bool().unwrap_or(false);
            let child = format!("{path}/{}", escape(name));
            match object.get(name) {
                None if !optional => issues.push(Issue {
                    severity: Severity::Error,
                    code: "types/missing-property",
                    path: child,
                    message: format!("the required property {name:?} is missing"),
                }),
                None => {}
                Some(Value::Null) if optional && !self.accepts_null(&property["type"]) => {
                    issues.push(Issue {
                        severity: Severity::Error,
                        code: "types/null-for-optional-property",
                        path: child,
                        message: format!(
                            "the optional property {name:?} is null; an optional property is omitted, and null is allowed only when its type includes null"
                        ),
                    })
                }
                Some(item) => self.check(&property["type"], item, &child, issues),
            }
        }
        if properties.is_empty() {
            return;
        }
        for key in object.keys() {
            if !properties.iter().any(|p| p["name"] == key.as_str()) {
                issues.push(Issue {
                    severity: Severity::Warning,
                    code: "types/unknown-property",
                    path: format!("{path}/{}", escape(key)),
                    message: format!("the specification defines no property {key:?} here"),
                });
            }
        }
    }

    fn accepts_null(&self, ty: &Value) -> bool {
        match ty["kind"].as_str() {
            Some("base") => ty["name"] == "null",
            Some("or") => ty["items"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|t| self.accepts_null(t)),
            Some("reference") => ty["name"]
                .as_str()
                .and_then(|n| self.aliases.get(n))
                .is_some_and(|alias| self.accepts_null(&alias["type"])),
            _ => false,
        }
    }

    fn check_enumeration(
        &self,
        name: &str,
        enumeration: &Value,
        value: &Value,
        path: &str,
        issues: &mut Vec<Issue>,
    ) {
        let base = enumeration["type"]["name"].as_str().unwrap_or_default();
        if let Err(expected) = check_base(base, value) {
            issues.push(Issue {
                severity: Severity::Error,
                code: "types/wrong-type",
                path: path.to_string(),
                message: format!("expected {expected} ({name}), got {}", describe(value)),
            });
            return;
        }
        let known = enumeration["values"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|v| &v["value"] == value);
        if !known
            && !enumeration["supportsCustomValues"]
                .as_bool()
                .unwrap_or(false)
        {
            issues.push(Issue {
                severity: Severity::Error,
                code: "types/unknown-enum-value",
                path: path.to_string(),
                message: format!("{value} is not a value of {name}"),
            });
        }
    }
}

fn check_base(name: &str, value: &Value) -> Result<(), &'static str> {
    let ok = match name {
        "string" | "URI" | "DocumentUri" => value.is_string(),
        "integer" => value
            .as_i64()
            .is_some_and(|n| (i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&n)),
        "uinteger" => value.as_u64().is_some_and(|n| n <= i32::MAX as u64),
        "decimal" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    };
    if ok {
        return Ok(());
    }
    Err(match name {
        "integer" => "an integer in [-2^31, 2^31-1]",
        "uinteger" => "an unsigned integer in [0, 2^31-1]",
        "decimal" => "a number",
        "boolean" => "a boolean",
        "null" => "null",
        _ => "a string",
    })
}

fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(_) => "a string".to_string(),
        Value::Array(_) => "an array".to_string(),
        Value::Object(_) => "an object".to_string(),
    }
}

fn type_name(ty: &Value) -> String {
    match ty["kind"].as_str() {
        Some("base") | Some("reference") => ty["name"].as_str().unwrap_or("?").to_string(),
        Some("array") => format!("{}[]", type_name(&ty["element"])),
        Some("stringLiteral") => ty["value"].to_string(),
        Some(kind) => kind.to_string(),
        None => "?".to_string(),
    }
}

fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// Whether the capability at a dotted `path` is declared: present and neither false nor null.
pub fn capability<'a>(capabilities: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = capabilities;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    match current {
        Value::Null | Value::Bool(false) => None,
        declared => Some(declared),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn codes(method: &str, value: Value) -> Vec<(&'static str, String)> {
        let def = model().message(method).unwrap();
        model()
            .validate(def.params.as_ref().unwrap(), &value, "/params")
            .into_iter()
            .map(|i| (i.code, i.path))
            .collect()
    }

    #[test]
    fn the_embedded_model_is_3_18() {
        assert_eq!(model().version, "3.18.0");
        let hover = model().message("textDocument/hover").unwrap();
        assert_eq!(hover.direction, MessageDirection::ClientToServer);
        assert_eq!(hover.server_capability.as_deref(), Some("hoverProvider"));
    }

    #[test]
    fn valid_params_have_no_issues() {
        let params = json!({"textDocument": {"uri": "file:///a.rs"}, "position": {"line": 0, "character": 3}});
        assert!(codes("textDocument/hover", params).is_empty());
    }

    #[test]
    fn reports_missing_null_wrong_and_unknown_properties() {
        let params = json!({
            "textDocument": {"uri": "file:///a.rs", "extra": 1},
            "position": {"line": -1},
            "workDoneToken": null
        });
        assert_eq!(
            codes("textDocument/hover", params),
            vec![
                (
                    "types/unknown-property",
                    "/params/textDocument/extra".to_string()
                ),
                ("types/wrong-type", "/params/position/line".to_string()),
                (
                    "types/missing-property",
                    "/params/position/character".to_string()
                ),
                (
                    "types/null-for-optional-property",
                    "/params/workDoneToken".to_string()
                ),
            ]
        );
    }

    #[test]
    fn checks_unions_and_enumerations() {
        let def = model().message("textDocument/hover").unwrap();
        let result = def.result.as_ref().unwrap();
        assert!(model().validate(result, &Value::Null, "/result").is_empty());
        assert!(
            model()
                .validate(result, &json!({"contents": "text"}), "/result")
                .is_empty()
        );
        let issues = model().validate(result, &json!({"contents": 3}), "/result");
        assert_eq!(issues[0].code, "types/no-alternative-matches");

        let diagnostic = json!({"uri": "file:///a", "diagnostics": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "message": "m", "severity": 9}]});
        assert_eq!(
            codes("textDocument/publishDiagnostics", diagnostic),
            vec![(
                "types/unknown-enum-value",
                "/params/diagnostics/0/severity".to_string()
            )]
        );
    }

    #[test]
    fn capability_paths() {
        let caps = json!({"hoverProvider": true, "textDocument": {"hover": {}}, "definitionProvider": false});
        assert!(capability(&caps, "hoverProvider").is_some());
        assert!(capability(&caps, "textDocument.hover").is_some());
        assert!(capability(&caps, "definitionProvider").is_none());
        assert!(capability(&caps, "workspace.applyEdit").is_none());
    }
}
