use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Finding {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub offset: u64,
}

impl Finding {
    pub fn new(
        severity: Severity,
        code: &'static str,
        message: impl Into<String>,
        offset: u64,
    ) -> Self {
        Finding {
            severity,
            code,
            message: message.into(),
            offset,
        }
    }
}
