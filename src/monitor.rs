//! The message log of one LSP session and every check on it. A monitor sees the messages of both
//! sides, whoever sends them: the strict client, the proxy or the strict server.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::document::{self, Document, Located};
use crate::finding::{Finding, Severity};
use crate::jsonrpc::{self, Kind};
use crate::model::{self, MessageDef, MessageDirection};
use crate::position::Encoding;
use crate::progress::{self, TokenState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    ClientToServer,
    ServerToClient,
    ServerStderr,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct LoggedMessage {
    pub session_id: u64,
    pub seq: u64,
    pub direction: Direction,
    /// The byte offset of the frame in its direction's stream; none for stderr lines.
    pub offset: Option<u64>,
    pub time_ms: f64,
    pub kind: Option<Kind>,
    pub method: Option<String>,
    pub id: Option<String>,
    pub body: String,
    pub findings: Vec<Finding>,
    pub paired_seq: Option<u64>,
    pub auto: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Running,
    Exited,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionInfo {
    pub id: u64,
    pub command: Vec<String>,
    pub state: SessionState,
    pub exit_code: Option<i32>,
    pub auto_respond: bool,
}

#[derive(Debug, Clone)]
pub enum Event {
    Message(Box<LoggedMessage>),
    Session(SessionInfo),
}

const NOTIFICATIONS_BEFORE_INITIALIZE_RESULT: &[&str] = &[
    "window/showMessage",
    "window/logMessage",
    "telemetry/event",
    "$/progress",
];

const REQUEST_CANCELLED: i64 = -32800;

#[derive(Debug, Clone)]
struct Pending {
    seq: u64,
    method: String,
    params: Value,
    /// The version of the request's `textDocument` when the request was sent.
    document_version: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenOrigin {
    /// Created by `window/workDoneProgress/create`; `ready` once the client answered.
    Created { ready: bool },
    /// A `workDoneToken` of a request.
    WorkDone,
    /// A `partialResultToken` of a request.
    PartialResult,
}

#[derive(Debug, Clone)]
struct Token {
    state: TokenState,
    origin: TokenOrigin,
    /// The key of the request that carried the token.
    request: Option<String>,
}

#[derive(Debug, Clone)]
struct TrackedDocument {
    document: Document,
    /// The text is no longer known after a change that could not be applied.
    lost: bool,
}

#[derive(Default)]
struct Protocol {
    /// Requests waiting for a response, by the side that sent them.
    pending_client: HashMap<String, Pending>,
    pending_server: HashMap<String, Pending>,
    cancelled: HashSet<String>,
    initialize_answered: bool,
    shutdown_sent: bool,
    client_capabilities: Value,
    server_capabilities: Value,
    encoding: Option<Encoding>,
    sync_kind: i64,
    registrations: HashMap<String, String>,
    documents: HashMap<String, TrackedDocument>,
    tokens: HashMap<String, Token>,
    /// Keys of `window/workDoneProgress/create` requests and their tokens.
    creating: HashMap<String, String>,
}

struct State {
    log: Vec<LoggedMessage>,
    protocol: Protocol,
    state: SessionState,
    exit_code: Option<i32>,
}

pub struct Monitor {
    id: u64,
    command: Vec<String>,
    auto_respond: bool,
    started: Instant,
    state: Mutex<State>,
    events: broadcast::Sender<Event>,
    record: Option<Arc<Mutex<File>>>,
}

impl Monitor {
    pub fn new(
        id: u64,
        command: Vec<String>,
        auto_respond: bool,
        events: broadcast::Sender<Event>,
        record: Option<Arc<Mutex<File>>>,
    ) -> Monitor {
        let monitor = Monitor {
            id,
            command,
            auto_respond,
            started: Instant::now(),
            state: Mutex::new(State {
                log: Vec::new(),
                protocol: Protocol::default(),
                state: SessionState::Running,
                exit_code: None,
            }),
            events,
            record,
        };
        let _ = monitor.events.send(Event::Session(monitor.info()));
        monitor
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn info(&self) -> SessionInfo {
        let state = self.state.lock().unwrap();
        SessionInfo {
            id: self.id,
            command: self.command.clone(),
            state: state.state,
            exit_code: state.exit_code,
            auto_respond: self.auto_respond,
        }
    }

    pub fn messages(&self, after_seq: Option<u64>) -> Vec<LoggedMessage> {
        let state = self.state.lock().unwrap();
        state
            .log
            .iter()
            .filter(|m| after_seq.is_none_or(|after| m.seq > after))
            .cloned()
            .collect()
    }

    /// Adds a finding to a logged message, for checks that need later messages to decide; the
    /// finding gets the message's offset, and the message is sent to subscribers again.
    pub fn add_finding(&self, seq: u64, mut finding: Finding) {
        let mut state = self.state.lock().unwrap();
        let Some(message) = state.log.get_mut(seq as usize - 1) else {
            return;
        };
        finding.offset = message.offset.unwrap_or_default();
        message.findings.push(finding);
        let message = message.clone();
        drop(state);
        let _ = self.events.send(Event::Message(Box::new(message)));
    }

    pub fn mark_exited(&self, exit_code: Option<i32>) {
        let mut state = self.state.lock().unwrap();
        if state.state == SessionState::Exited {
            return;
        }
        state.state = SessionState::Exited;
        state.exit_code = exit_code;
        drop(state);
        let _ = self.events.send(Event::Session(self.info()));
    }

    /// The text of an open document as the monitor reconstructed it.
    pub fn document(&self, uri: &str) -> Option<Document> {
        let state = self.state.lock().unwrap();
        state
            .protocol
            .documents
            .get(uri)
            .filter(|d| !d.lost)
            .map(|d| d.document.clone())
    }

    pub fn encoding(&self) -> Encoding {
        self.state
            .lock()
            .unwrap()
            .protocol
            .encoding
            .unwrap_or(Encoding::Utf16)
    }

    pub fn observe_stderr(&self, line: String) -> LoggedMessage {
        let state = self.state.lock().unwrap();
        let message = self.message(&state, Direction::ServerStderr, line, Vec::new(), false);

        self.publish(state, message)
    }

    /// Logs and checks one message; `findings` are those found while reading its frame.
    pub fn observe(
        &self,
        direction: Direction,
        body: String,
        offset: u64,
        mut findings: Vec<Finding>,
        auto: bool,
    ) -> LoggedMessage {
        let mut state = self.state.lock().unwrap();
        let seq = state.log.len() as u64 + 1;
        let mut kind = None;
        let mut method = None;
        let mut id = None;
        let mut paired_seq = None;
        if !body.is_empty() {
            match serde_json::from_str::<Value>(&body) {
                Err(err) => findings.push(Finding::new(
                    Severity::Error,
                    "jsonrpc/not-json",
                    format!("the content is not JSON: {err}"),
                    offset,
                )),
                Ok(value) => {
                    let classified = jsonrpc::classify(&value, offset, &mut findings);
                    let mut check = Check {
                        protocol: &mut state.protocol,
                        findings: &mut findings,
                        offset,
                        from_client: direction == Direction::ClientToServer,
                    };
                    let paired = check.message(&classified, &value, seq);
                    kind = Some(classified.kind);
                    method = classified
                        .method
                        .clone()
                        .or_else(|| paired.as_ref().map(|p| p.method.clone()));
                    paired_seq = paired.map(|p| p.seq);
                    id = classified.id.as_ref().map(jsonrpc::id_key);
                }
            }
        }
        let mut message = self.message(&state, direction, body, findings, auto);
        message.kind = kind;
        message.method = method;
        message.id = id;
        message.paired_seq = paired_seq;
        message.offset = Some(offset);
        self.publish(state, message)
    }

    fn message(
        &self,
        state: &State,
        direction: Direction,
        body: String,
        findings: Vec<Finding>,
        auto: bool,
    ) -> LoggedMessage {
        LoggedMessage {
            session_id: self.id,
            seq: state.log.len() as u64 + 1,
            offset: None,
            direction,
            time_ms: self.started.elapsed().as_secs_f64() * 1000.0,
            kind: None,
            method: None,
            id: None,
            body,
            findings,
            paired_seq: None,
            auto,
        }
    }

    fn publish(
        &self,
        mut state: std::sync::MutexGuard<'_, State>,
        message: LoggedMessage,
    ) -> LoggedMessage {
        if let Some(record) = &self.record
            && let Ok(line) = serde_json::to_string(&message)
        {
            let _ = writeln!(record.lock().unwrap(), "{line}");
        }
        state.log.push(message.clone());
        drop(state);
        let _ = self.events.send(Event::Message(Box::new(message.clone())));
        message
    }
}

struct Check<'a> {
    protocol: &'a mut Protocol,
    findings: &'a mut Vec<Finding>,
    offset: u64,
    from_client: bool,
}

fn token_key(value: &Value) -> Option<String> {
    (value.is_string() || value.is_number()).then(|| value.to_string())
}

fn text_document_uri(params: &Value) -> Option<&str> {
    params["textDocument"]["uri"].as_str()
}

impl Check<'_> {
    fn push(&mut self, severity: Severity, code: &'static str, message: String) {
        self.findings
            .push(Finding::new(severity, code, message, self.offset));
    }

    fn error(&mut self, code: &'static str, message: String) {
        self.push(Severity::Error, code, message);
    }

    fn warning(&mut self, code: &'static str, message: String) {
        self.push(Severity::Warning, code, message);
    }

    fn issues(&mut self, issues: Vec<model::Issue>) {
        for issue in issues {
            self.push(
                issue.severity,
                issue.code,
                format!("{}: {}", issue.path, issue.message),
            );
        }
    }

    /// Checks one parsed message and returns the request it answers, if any.
    fn message(
        &mut self,
        message: &jsonrpc::Classified,
        value: &Value,
        seq: u64,
    ) -> Option<Pending> {
        let method = message.method.clone().unwrap_or_default();
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        self.lifecycle(message.kind, &method);
        let def = model::model().message(&method);

        match message.kind {
            Kind::Request | Kind::Notification => {
                self.types(message.kind, &method, def, value);
                if let Some(def) = def {
                    self.capability(message.kind, def, &params);
                }
            }
            Kind::Response | Kind::Invalid => {}
        }

        let paired = self.pairing(message, &method, &params, seq);

        match message.kind {
            Kind::Request => self.on_request(&method, &params, message.id.as_ref()),
            Kind::Notification => self.on_notification(&method, &params),
            Kind::Response => {
                if let Some(pending) = &paired {
                    self.on_response(pending, value, message.id.as_ref());
                }
            }
            Kind::Invalid => {}
        }
        paired
    }

    fn lifecycle(&mut self, kind: Kind, method: &str) {
        let p = &self.protocol;
        if self.from_client {
            if p.shutdown_sent && !(kind == Kind::Notification && method == "exit") {
                self.error(
                    "lifecycle/client-message-after-shutdown",
                    "after a shutdown request the client may only send the exit notification"
                        .to_string(),
                );
            } else if !p.initialize_answered
                && !(kind == Kind::Request && method == "initialize")
                && !(kind == Kind::Notification && method == "exit")
            {
                self.error(
                    "lifecycle/client-message-before-initialize-result",
                    "the client must not send requests or notifications before the server answers initialize".to_string(),
                );
            }
        } else if !p.initialize_answered {
            let allowed = match kind {
                Kind::Notification => NOTIFICATIONS_BEFORE_INITIALIZE_RESULT.contains(&method),
                Kind::Request => method == "window/showMessageRequest",
                Kind::Response | Kind::Invalid => true,
            };
            if !allowed {
                self.error(
                    "lifecycle/server-message-before-initialize-result",
                    format!("the server must not send {method} before it answers initialize"),
                );
            }
        }
    }

    fn types(&mut self, kind: Kind, method: &str, def: Option<&MessageDef>, value: &Value) {
        let Some(def) = def else {
            if !method.starts_with("$/") {
                self.warning(
                    "types/unknown-method",
                    format!("the specification defines no method {method}"),
                );
            }
            return;
        };
        let expected_direction = if self.from_client {
            MessageDirection::ClientToServer
        } else {
            MessageDirection::ServerToClient
        };
        if def.direction != MessageDirection::Both && def.direction != expected_direction {
            self.error(
                "types/wrong-direction",
                format!(
                    "{method} is sent from the {}",
                    if def.direction == MessageDirection::ClientToServer {
                        "client to the server"
                    } else {
                        "server to the client"
                    }
                ),
            );
        }
        if def.is_request != (kind == Kind::Request) {
            self.error(
                "types/wrong-message-kind",
                format!(
                    "{method} is a {}",
                    if def.is_request {
                        "request"
                    } else {
                        "notification"
                    }
                ),
            );
        }
        match (&def.params, value.get("params")) {
            (Some(ty), Some(params)) => {
                let issues = model::model().validate(ty, params, "/params");
                self.issues(issues);
            }
            (Some(_), None) => {
                self.error("types/missing-params", format!("{method} requires params"))
            }
            (None, Some(_)) => self.warning(
                "types/unexpected-params",
                format!("{method} takes no params"),
            ),
            (None, None) => {}
        }
    }

    fn server_declares(&self, path: &str) -> bool {
        let caps = &self.protocol.server_capabilities;
        if let Some(rest) = path.strip_prefix("textDocumentSync")
            && let Some(kind) = caps["textDocumentSync"].as_i64()
        {
            return match rest {
                "" | ".openClose" | ".change" => kind > 0,
                _ => false,
            };
        }
        model::capability(caps, path).is_some()
    }

    fn capability(&mut self, kind: Kind, def: &MessageDef, params: &Value) {
        if !self.protocol.initialize_answered
            || (def.direction == MessageDirection::ClientToServer) != self.from_client
                && def.direction != MessageDirection::Both
        {
            return;
        }
        let severity = if kind == Kind::Request {
            Severity::Error
        } else {
            Severity::Warning
        };
        if self.from_client {
            let Some(path) = &def.server_capability else {
                return;
            };
            let registration = def.registration_method.as_deref().unwrap_or(&def.method);
            let registered = self
                .protocol
                .registrations
                .values()
                .any(|m| m == registration);
            if !self.server_declares(path) && !registered {
                self.push(
                    severity,
                    "capability/server-did-not-declare",
                    format!(
                        "the server declared neither {path} nor a dynamic registration of {registration}"
                    ),
                );
            }
        } else {
            if let Some(path) = &def.client_capability
                && model::capability(&self.protocol.client_capabilities, path).is_none()
            {
                self.push(
                    severity,
                    "capability/client-did-not-declare",
                    format!("the client did not declare {path}"),
                );
            }
            if def.method == "client/registerCapability" {
                self.dynamic_registrations(params);
            }
        }
    }

    fn dynamic_registrations(&mut self, params: &Value) {
        for registration in params["registrations"].as_array().into_iter().flatten() {
            let Some(method) = registration["method"].as_str() else {
                continue;
            };
            let Some(path) = model::model()
                .registered_by(method)
                .find_map(|def| def.client_capability.clone())
            else {
                continue;
            };
            let caps = &self.protocol.client_capabilities;
            let supported = std::iter::successors(Some(path.as_str()), |p| {
                p.rsplit_once('.').map(|(parent, _)| parent)
            })
            .any(|p| {
                model::capability(caps, &format!("{p}.dynamicRegistration"))
                    .is_some_and(|v| v == &Value::Bool(true))
            });
            if !supported {
                self.error(
                    "capability/dynamic-registration-not-supported",
                    format!("the client did not declare {path}.dynamicRegistration for {method}"),
                );
            }
        }
    }

    fn pairing(
        &mut self,
        message: &jsonrpc::Classified,
        method: &str,
        params: &Value,
        seq: u64,
    ) -> Option<Pending> {
        let key = message.id.as_ref().map(jsonrpc::id_key)?;
        let from_client = self.from_client;
        match message.kind {
            Kind::Request => {
                let document_version = text_document_uri(params)
                    .and_then(|uri| self.protocol.documents.get(uri))
                    .map(|d| d.document.version);
                let own = if from_client {
                    &mut self.protocol.pending_client
                } else {
                    &mut self.protocol.pending_server
                };
                let duplicate = own.contains_key(&key);
                own.insert(
                    key.clone(),
                    Pending {
                        seq,
                        method: method.to_string(),
                        params: params.clone(),
                        document_version,
                    },
                );
                if duplicate {
                    self.error(
                        "jsonrpc/duplicate-request-id",
                        format!("the id {key} is already used by a request without a response"),
                    );
                }
                None
            }
            Kind::Response => {
                let other = if from_client {
                    &mut self.protocol.pending_server
                } else {
                    &mut self.protocol.pending_client
                };
                match other.remove(&key) {
                    Some(pending) => Some(pending),
                    None => {
                        self.error(
                            "jsonrpc/response-to-unknown-request",
                            format!("no request with the id {key} is waiting for a response"),
                        );
                        None
                    }
                }
            }
            Kind::Notification | Kind::Invalid => None,
        }
    }

    fn on_request(&mut self, method: &str, params: &Value, id: Option<&Value>) {
        let key = id.map(jsonrpc::id_key);
        match method {
            "initialize" if self.from_client => {
                self.protocol.client_capabilities = params["capabilities"].clone();
            }
            "shutdown" if self.from_client => self.protocol.shutdown_sent = true,
            "client/registerCapability" if !self.from_client => {
                for r in params["registrations"].as_array().into_iter().flatten() {
                    if let (Some(id), Some(m)) = (r["id"].as_str(), r["method"].as_str()) {
                        self.protocol
                            .registrations
                            .insert(id.to_string(), m.to_string());
                    }
                }
            }
            "client/unregisterCapability" if !self.from_client => {
                for r in params["unregisterations"].as_array().into_iter().flatten() {
                    if let Some(id) = r["id"].as_str() {
                        self.protocol.registrations.remove(id);
                    }
                }
            }
            "window/workDoneProgress/create" if !self.from_client => {
                if let (Some(token), Some(key)) = (token_key(&params["token"]), key.clone()) {
                    if self
                        .protocol
                        .tokens
                        .get(&token)
                        .is_some_and(|t| t.state != TokenState::Ended)
                    {
                        self.error(
                            "progress/token-already-exists",
                            format!("the progress token {token} is already in use"),
                        );
                    }
                    self.protocol.tokens.insert(
                        token.clone(),
                        Token {
                            state: TokenState::Created,
                            origin: TokenOrigin::Created { ready: false },
                            request: None,
                        },
                    );
                    self.protocol.creating.insert(key, token);
                }
            }
            "workspace/applyEdit" if !self.from_client => {
                self.check_ranges_in(&params["edit"], None, None, "/params/edit");
            }
            _ => {}
        }
        for (field, origin) in [
            ("workDoneToken", TokenOrigin::WorkDone),
            ("partialResultToken", TokenOrigin::PartialResult),
        ] {
            if let Some(token) = token_key(&params[field]) {
                self.protocol.tokens.insert(
                    token,
                    Token {
                        state: TokenState::Created,
                        origin,
                        request: key.clone(),
                    },
                );
            }
        }
    }

    fn on_response(&mut self, pending: &Pending, value: &Value, id: Option<&Value>) {
        let key = id.map(jsonrpc::id_key).unwrap_or_default();
        let def = model::model().message(&pending.method);
        if let (Some(result), Some(ty)) = (value.get("result"), def.and_then(|d| d.result.as_ref()))
        {
            let issues = model::model().validate(ty, result, "/result");
            self.issues(issues);
        }
        let cancelled = self.protocol.cancelled.remove(&key);
        if value["error"]["code"].as_i64() == Some(REQUEST_CANCELLED) && !cancelled {
            self.warning(
                "cancel/cancelled-without-request",
                "the response is RequestCancelled, but the request was not cancelled".to_string(),
            );
        }
        let succeeded = value.get("result").is_some();
        match pending.method.as_str() {
            "initialize" if !self.from_client => {
                self.protocol.initialize_answered = true;
                if succeeded {
                    self.initialize_result(&value["result"]);
                }
            }
            "window/workDoneProgress/create" if self.from_client => {
                if let Some(token) = self.protocol.creating.remove(&key)
                    && let Some(t) = self.protocol.tokens.get_mut(&token)
                {
                    if succeeded {
                        t.origin = TokenOrigin::Created { ready: true };
                    } else {
                        self.protocol.tokens.remove(&token);
                    }
                }
            }
            _ => {}
        }
        if !self.from_client
            && succeeded
            && let Some(uri) = text_document_uri(&pending.params)
        {
            let unchanged = self
                .protocol
                .documents
                .get(uri)
                .is_some_and(|d| Some(d.document.version) == pending.document_version);
            if unchanged {
                let uri = uri.to_string();
                self.check_ranges_in(&value["result"], Some(&uri), Some(&uri), "/result");
            }
        }
        let request = Some(key);
        self.protocol
            .tokens
            .retain(|_, t| !(t.request == request && t.origin == TokenOrigin::PartialResult));
    }

    fn initialize_result(&mut self, result: &Value) {
        let caps = result["capabilities"].clone();
        let offered: Vec<String> =
            self.protocol.client_capabilities["general"]["positionEncodings"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_else(|| vec!["utf-16".to_string()]);
        let chosen = caps["positionEncoding"]
            .as_str()
            .unwrap_or("utf-16")
            .to_string();
        if chosen != "utf-16" && !offered.contains(&chosen) {
            self.error(
                "capability/position-encoding-not-offered",
                format!("the server chose {chosen}, which the client did not offer ({offered:?})"),
            );
        }
        let encoding = match document::encoding_of(&chosen) {
            Some(encoding) => encoding,
            None => {
                self.warning(
                    "capability/unknown-position-encoding",
                    format!("lsp-touchstone does not know the position encoding {chosen}; positions are read as utf-16"),
                );
                Encoding::Utf16
            }
        };
        self.protocol.encoding = Some(encoding);
        self.protocol.sync_kind = caps["textDocumentSync"]
            .as_i64()
            .or_else(|| caps["textDocumentSync"]["change"].as_i64())
            .unwrap_or(0);
        self.protocol.server_capabilities = caps;
    }

    fn on_notification(&mut self, method: &str, params: &Value) {
        match method {
            "textDocument/didOpen" if self.from_client => self.did_open(params),
            "textDocument/didChange" if self.from_client => self.did_change(params),
            "textDocument/didClose" if self.from_client => {
                if let Some(uri) = text_document_uri(params)
                    && self.protocol.documents.remove(uri).is_none()
                {
                    self.error(
                        "sync/not-open",
                        format!("{uri} is closed without being open"),
                    );
                }
            }
            "textDocument/didSave" if self.from_client => self.did_save(params),
            "textDocument/publishDiagnostics" if !self.from_client => {
                self.publish_diagnostics(params)
            }
            "$/cancelRequest" => {
                if let Some(key) = token_key(&params["id"]) {
                    let own = if self.from_client {
                        &self.protocol.pending_client
                    } else {
                        &self.protocol.pending_server
                    };
                    if own.contains_key(&key) {
                        self.protocol.cancelled.insert(key);
                    } else {
                        self.warning(
                            "cancel/unknown-request",
                            format!("no request with the id {key} is waiting; it may have been answered already"),
                        );
                    }
                }
            }
            "$/progress" if !self.from_client => self.progress(params),
            _ => {}
        }
    }

    fn did_open(&mut self, params: &Value) {
        let doc = &params["textDocument"];
        let (Some(uri), Some(version), Some(text)) = (
            doc["uri"].as_str(),
            doc["version"].as_i64(),
            doc["text"].as_str(),
        ) else {
            return;
        };
        if self.protocol.documents.contains_key(uri) {
            self.error(
                "sync/already-open",
                format!("{uri} is opened again without being closed"),
            );
        }
        self.protocol.documents.insert(
            uri.to_string(),
            TrackedDocument {
                document: Document {
                    version,
                    text: text.to_string(),
                },
                lost: false,
            },
        );
    }

    fn did_change(&mut self, params: &Value) {
        let (Some(uri), Some(version)) = (
            text_document_uri(params),
            params["textDocument"]["version"].as_i64(),
        ) else {
            return;
        };
        let encoding = self.protocol.encoding.unwrap_or(Encoding::Utf16);
        let sync_kind = self.protocol.sync_kind;
        let Some(mut tracked) = self.protocol.documents.remove(uri) else {
            self.error(
                "sync/not-open",
                format!("{uri} is changed without being open"),
            );
            return;
        };
        if version <= tracked.document.version {
            self.error(
                "sync/version-not-increasing",
                format!(
                    "the version {version} is not greater than the previous version {}",
                    tracked.document.version
                ),
            );
        }
        tracked.document.version = version;
        for (i, change) in params["contentChanges"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let Some(text) = change["text"].as_str() else {
                continue;
            };
            let Some(range) = change.get("range") else {
                tracked.document.text = text.to_string();
                tracked.lost = false;
                continue;
            };
            if sync_kind == 1 {
                self.error(
                    "sync/incremental-not-negotiated",
                    format!("contentChanges/{i} has a range, but the server asked for full text (TextDocumentSyncKind.Full)"),
                );
            }
            if tracked.lost {
                continue;
            }
            let path = format!("/params/contentChanges/{i}/range");
            match self.locate_range(&tracked.document.text, encoding, range, &path, "sync") {
                Some((start, end)) => {
                    if let Some(length) = change["rangeLength"].as_u64() {
                        let units = units(&tracked.document.text[start..end], encoding);
                        if units != length {
                            self.warning(
                                "sync/range-length-mismatch",
                                format!("{path}: rangeLength is {length}, but the range spans {units} code units"),
                            );
                        }
                    }
                    tracked.document.text.replace_range(start..end, text);
                }
                None => tracked.lost = true,
            }
        }
        self.protocol.documents.insert(uri.to_string(), tracked);
    }

    fn did_save(&mut self, params: &Value) {
        let Some(uri) = text_document_uri(params) else {
            return;
        };
        let Some(tracked) = self.protocol.documents.get(uri) else {
            self.error(
                "sync/not-open",
                format!("{uri} is saved without being open"),
            );
            return;
        };
        if let Some(text) = params["text"].as_str()
            && !tracked.lost
            && text != tracked.document.text
        {
            self.error(
                "sync/save-text-mismatch",
                "the saved text differs from the text the changes so far produce".to_string(),
            );
        }
    }

    fn publish_diagnostics(&mut self, params: &Value) {
        let Some(uri) = params["uri"].as_str() else {
            return;
        };
        let Some(tracked) = self.protocol.documents.get(uri) else {
            return;
        };
        if tracked.lost {
            return;
        }
        let current = tracked.document.version;
        if let Some(version) = params["version"].as_i64() {
            if version > current {
                self.error(
                    "sync/diagnostics-for-future-version",
                    format!("the diagnostics are for version {version}, but the document is at {current}"),
                );
                return;
            }
            if version < current {
                self.warning(
                    "sync/diagnostics-for-old-version",
                    format!(
                        "the diagnostics are for version {version}; the document is at {current}"
                    ),
                );
                return;
            }
        }
        let uri = uri.to_string();
        self.check_ranges_in(
            &params["diagnostics"],
            Some(&uri),
            None,
            "/params/diagnostics",
        );
    }

    fn progress(&mut self, params: &Value) {
        let Some(token) = token_key(&params["token"]) else {
            return;
        };
        let Some(entry) = self.protocol.tokens.get(&token).cloned() else {
            self.error(
                "progress/unknown-token",
                format!("the progress token {token} was neither created nor given in a request"),
            );
            return;
        };
        if entry.origin == TokenOrigin::PartialResult {
            return;
        }
        if entry.origin == (TokenOrigin::Created { ready: false }) {
            self.warning(
                "progress/before-create-response",
                format!("progress on {token} is reported before the client answered window/workDoneProgress/create"),
            );
        }
        let Some(kind) = params["value"]["kind"].as_str() else {
            return;
        };
        let Some(progress_kind) = progress::kind_of(kind) else {
            self.error(
                "progress/unknown-kind",
                format!("{kind:?} is not begin, report or end"),
            );
            return;
        };
        match progress::step(entry.state, progress_kind) {
            Some(next) => {
                if let Some(t) = self.protocol.tokens.get_mut(&token) {
                    t.state = next;
                }
            }
            None => self.error(
                "progress/out-of-order",
                format!(
                    "{kind} is not allowed after {} on the token {token}",
                    match entry.state {
                        TokenState::Created => "its creation",
                        TokenState::Begun => "begin",
                        TokenState::Ended => "end",
                    }
                ),
            ),
        }
    }

    /// Locates a range in `text`; reports unusable positions and returns `None` for them.
    fn locate_range(
        &mut self,
        text: &str,
        encoding: Encoding,
        range: &Value,
        path: &str,
        area: &str,
    ) -> Option<(usize, usize)> {
        let mut bytes = [0usize; 2];
        for (i, end) in ["start", "end"].iter().enumerate() {
            let position = &range[end];
            let (Some(line), Some(character)) =
                (position["line"].as_u64(), position["character"].as_u64())
            else {
                return None;
            };
            let at = format!("{path}/{end}");
            bytes[i] = match document::locate(text, encoding, line, character) {
                Located::Exact(b) => b,
                Located::Clamped(b) => {
                    self.warning(
                        if area == "sync" {
                            "sync/character-past-line-end"
                        } else {
                            "positions/character-past-line-end"
                        },
                        format!("{at}: character {character} is past the end of line {line}; the specification clamps it to the line length"),
                    );
                    b
                }
                Located::SplitsChar => {
                    self.error(
                        if area == "sync" {
                            "sync/position-splits-character"
                        } else {
                            "positions/splits-character"
                        },
                        format!("{at}: character {character} on line {line} is inside a character in {encoding:?}"),
                    );
                    return None;
                }
                Located::LineOutOfRange => {
                    self.error(
                        if area == "sync" {
                            "sync/line-out-of-range"
                        } else {
                            "positions/line-out-of-range"
                        },
                        format!("{at}: line {line} does not exist"),
                    );
                    return None;
                }
            };
        }
        if bytes[0] > bytes[1] {
            self.error(
                if area == "sync" {
                    "sync/range-reversed"
                } else {
                    "positions/range-reversed"
                },
                format!("{path}: the start is after the end"),
            );
            return None;
        }
        Some((bytes[0], bytes[1]))
    }

    /// Checks every range in `value` against the open document it refers to: the `uri`,
    /// `targetUri` or `textDocument.uri` of an enclosing object, a key of `changes`, or `context`.
    fn check_ranges_in(
        &mut self,
        value: &Value,
        context: Option<&str>,
        origin: Option<&str>,
        path: &str,
    ) {
        let mut ranges = Vec::new();
        collect_ranges(value, context, origin, path, &mut ranges);
        let encoding = self.protocol.encoding.unwrap_or(Encoding::Utf16);
        for (uri, range, at) in ranges {
            let Some(tracked) = self.protocol.documents.get(&uri) else {
                continue;
            };
            if tracked.lost {
                continue;
            }
            let text = tracked.document.text.clone();
            self.locate_range(&text, encoding, &range, &at, "positions");
        }
    }
}

fn units(text: &str, encoding: Encoding) -> u64 {
    (match encoding {
        Encoding::Utf8 => text.len(),
        Encoding::Utf16 => text.encode_utf16().count(),
        Encoding::Utf32 => text.chars().count(),
    }) as u64
}

fn is_range(value: &Value) -> bool {
    ["start", "end"]
        .iter()
        .all(|k| value[k]["line"].is_u64() && value[k]["character"].is_u64())
}

fn collect_ranges(
    value: &Value,
    context: Option<&str>,
    origin: Option<&str>,
    path: &str,
    out: &mut Vec<(String, Value, String)>,
) {
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                collect_ranges(item, context, origin, &format!("{path}/{i}"), out);
            }
        }
        Value::Object(object) => {
            let own = object
                .get("uri")
                .or_else(|| object.get("targetUri"))
                .or_else(|| object.get("textDocument").and_then(|d| d.get("uri")))
                .and_then(Value::as_str);
            let context = own.or(context);
            for (key, item) in object {
                let at = format!("{path}/{key}");
                let scope = if key == "originSelectionRange" {
                    origin
                } else {
                    context
                };
                if is_range(item) {
                    if let Some(uri) = scope {
                        out.push((uri.to_string(), item.clone(), at));
                    }
                } else if key == "changes" && item.is_object() {
                    for (uri, edits) in item.as_object().unwrap() {
                        collect_ranges(edits, Some(uri), origin, &format!("{at}/{uri}"), out);
                    }
                } else {
                    collect_ranges(item, scope, origin, &at, out);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fixture {
        monitor: Monitor,
        next_id: i64,
    }

    impl Fixture {
        fn new(client: Value, server: Value) -> Fixture {
            let (events, _) = broadcast::channel(64);
            let mut fixture = Fixture {
                monitor: Monitor::new(1, vec![], false, events, None),
                next_id: 100,
            };
            fixture.client(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"processId": null, "rootUri": null, "capabilities": client}}));
            fixture.server(json!({"jsonrpc": "2.0", "id": 1, "result": {"capabilities": server}}));
            fixture.client(json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
            fixture
        }

        fn send(&mut self, direction: Direction, message: Value) -> Vec<&'static str> {
            self.monitor
                .observe(direction, message.to_string(), 0, Vec::new(), false)
                .findings
                .iter()
                .map(|f| f.code)
                .collect()
        }

        fn client(&mut self, message: Value) -> Vec<&'static str> {
            self.send(Direction::ClientToServer, message)
        }

        fn server(&mut self, message: Value) -> Vec<&'static str> {
            self.send(Direction::ServerToClient, message)
        }

        fn client_request(&mut self, method: &str, params: Value) -> Vec<&'static str> {
            self.next_id += 1;
            let id = self.next_id;
            self.client(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
        }

        fn notify(&mut self, method: &str, params: Value) -> Vec<&'static str> {
            self.client(json!({"jsonrpc": "2.0", "method": method, "params": params}))
        }

        fn open(&mut self, text: &str) {
            assert!(
                self.notify(
                    "textDocument/didOpen",
                    json!({"textDocument": {"uri": "file:///a", "languageId": "rust", "version": 1, "text": text}})
                )
                .is_empty()
            );
        }
    }

    fn range(l1: u64, c1: u64, l2: u64, c2: u64) -> Value {
        json!({"start": {"line": l1, "character": c1}, "end": {"line": l2, "character": c2}})
    }

    #[test]
    fn types_are_checked_against_the_model() {
        let mut f = Fixture::new(json!({}), json!({"hoverProvider": true}));
        assert_eq!(
            f.client_request(
                "textDocument/hover",
                json!({"textDocument": {"uri": "file:///a"}})
            ),
            vec!["types/missing-property"]
        );
        assert_eq!(
            f.client(json!({"jsonrpc": "2.0", "method": "textDocument/hover", "params": {}})),
            vec![
                "types/wrong-message-kind",
                "types/missing-property",
                "types/missing-property"
            ]
        );
        assert_eq!(
            f.server(json!({"jsonrpc": "2.0", "id": 5, "method": "textDocument/hover", "params": {"textDocument": {"uri": "file:///a"}, "position": {"line": 0, "character": 0}}})),
            vec!["types/wrong-direction"]
        );
        assert_eq!(
            f.notify("x/custom", json!({})),
            vec!["types/unknown-method"]
        );
        assert!(f.notify("$/custom", json!({})).is_empty());
    }

    #[test]
    fn results_are_checked_against_the_request() {
        let mut f = Fixture::new(json!({}), json!({"hoverProvider": true}));
        f.client_request(
            "textDocument/hover",
            json!({"textDocument": {"uri": "file:///a"}, "position": {"line": 0, "character": 0}}),
        );
        assert_eq!(
            f.server(json!({"jsonrpc": "2.0", "id": f.next_id, "result": {"contents": 1}})),
            vec!["types/no-alternative-matches"]
        );
    }

    #[test]
    fn capabilities_are_checked_both_ways() {
        let mut f = Fixture::new(
            json!({"workspace": {"didChangeWatchedFiles": {"dynamicRegistration": false}}}),
            json!({}),
        );
        assert_eq!(
            f.client_request(
                "textDocument/hover",
                json!({"textDocument": {"uri": "file:///a"}, "position": {"line": 0, "character": 0}})
            ),
            vec!["capability/server-did-not-declare"]
        );
        assert_eq!(
            f.server(json!({"jsonrpc": "2.0", "id": 7, "method": "workspace/diagnostic/refresh"})),
            vec!["capability/client-did-not-declare"]
        );
        assert_eq!(
            f.server(json!({"jsonrpc": "2.0", "id": 8, "method": "client/registerCapability", "params": {"registrations": [{"id": "r", "method": "workspace/didChangeWatchedFiles", "registerOptions": {"watchers": []}}]}})),
            vec!["capability/dynamic-registration-not-supported"]
        );
    }

    #[test]
    fn a_dynamic_registration_declares_the_server_capability() {
        let mut f = Fixture::new(
            json!({"textDocument": {"hover": {"dynamicRegistration": true}}}),
            json!({}),
        );
        assert!(
            f.server(json!({"jsonrpc": "2.0", "id": 8, "method": "client/registerCapability", "params": {"registrations": [{"id": "r", "method": "textDocument/hover"}]}}))
                .is_empty()
        );
        assert!(
            f.client_request(
                "textDocument/hover",
                json!({"textDocument": {"uri": "file:///a"}, "position": {"line": 0, "character": 0}})
            )
            .is_empty()
        );
    }

    #[test]
    fn the_position_encoding_must_be_offered() {
        let (events, _) = broadcast::channel(64);
        let monitor = Monitor::new(1, vec![], false, events, None);
        monitor.observe(
            Direction::ClientToServer,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"processId": null, "rootUri": null, "capabilities": {"general": {"positionEncodings": ["utf-16"]}}}}).to_string(),
            0,
            Vec::new(),
            false,
        );
        let answer = monitor.observe(
            Direction::ServerToClient,
            json!({"jsonrpc": "2.0", "id": 1, "result": {"capabilities": {"positionEncoding": "utf-8"}}}).to_string(),
            0,
            Vec::new(),
            false,
        );
        assert_eq!(
            answer.findings[0].code,
            "capability/position-encoding-not-offered"
        );
    }

    #[test]
    fn document_changes_are_applied_and_checked() {
        let mut f = Fixture::new(
            json!({}),
            json!({"textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": true}}}),
        );
        f.open("a😀b\nline");
        let change = |version: i64, range: Value, text: &str| json!({"textDocument": {"uri": "file:///a", "version": version}, "contentChanges": [{"range": range, "text": text}]});
        assert_eq!(
            f.notify("textDocument/didChange", change(2, range(0, 2, 0, 2), "x")),
            vec!["sync/position-splits-character"]
        );
        f.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": "file:///a"}}),
        );
        f.open("a😀b\nline");
        assert!(
            f.notify("textDocument/didChange", change(2, range(0, 1, 0, 3), "X"))
                .is_empty()
        );
        assert_eq!(f.monitor.document("file:///a").unwrap().text, "aXb\nline");
        assert_eq!(
            f.notify("textDocument/didChange", change(2, range(1, 0, 1, 0), "")),
            vec!["sync/version-not-increasing"]
        );
        assert_eq!(
            f.notify("textDocument/didChange", change(4, range(1, 9, 1, 9), "!")),
            vec![
                "sync/character-past-line-end",
                "sync/character-past-line-end"
            ]
        );
        assert_eq!(f.monitor.document("file:///a").unwrap().text, "aXb\nline!");
        assert_eq!(
            f.notify(
                "textDocument/didSave",
                json!({"textDocument": {"uri": "file:///a"}, "text": "other"})
            ),
            vec!["sync/save-text-mismatch"]
        );
        assert_eq!(
            f.notify("textDocument/didOpen", json!({"textDocument": {"uri": "file:///a", "languageId": "rust", "version": 1, "text": ""}})),
            vec!["sync/already-open"]
        );
    }

    #[test]
    fn server_positions_are_checked_against_the_document() {
        let mut f = Fixture::new(
            json!({"textDocument": {"publishDiagnostics": {}}}),
            json!({"textDocumentSync": 2, "definitionProvider": true}),
        );
        f.open("a😀b");
        let diagnostics = |version: i64, range: Value| json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": "file:///a", "version": version, "diagnostics": [{"range": range, "message": "m"}]}});
        assert_eq!(
            f.server(diagnostics(1, range(0, 2, 0, 3))),
            vec!["positions/splits-character"]
        );
        assert_eq!(
            f.server(diagnostics(1, range(3, 0, 3, 0))),
            vec!["positions/line-out-of-range"]
        );
        assert_eq!(
            f.server(diagnostics(2, range(0, 0, 0, 0))),
            vec!["sync/diagnostics-for-future-version"]
        );
        f.client_request(
            "textDocument/definition",
            json!({"textDocument": {"uri": "file:///a"}, "position": {"line": 0, "character": 0}}),
        );
        assert_eq!(
            f.server(json!({"jsonrpc": "2.0", "id": f.next_id, "result": [{"uri": "file:///a", "range": range(0, 3, 0, 1)}]})),
            vec!["positions/range-reversed"]
        );
    }

    #[test]
    fn progress_follows_the_proved_state_machine() {
        let mut f = Fixture::new(json!({"window": {"workDoneProgress": true}}), json!({}));
        let progress = |token: &str, kind: &str| json!({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": token, "value": {"kind": kind, "title": "t"}}});
        assert_eq!(
            f.server(progress("x", "begin")),
            vec!["progress/unknown-token"]
        );
        f.server(json!({"jsonrpc": "2.0", "id": 9, "method": "window/workDoneProgress/create", "params": {"token": "t"}}));
        assert_eq!(
            f.server(progress("t", "begin")),
            vec!["progress/before-create-response"]
        );
        f.client(json!({"jsonrpc": "2.0", "id": 9, "result": null}));
        assert!(f.server(progress("t", "report")).is_empty());
        assert!(f.server(progress("t", "end")).is_empty());
        assert_eq!(
            f.server(progress("t", "report")),
            vec!["progress/out-of-order"]
        );
    }

    #[test]
    fn cancellation_is_tracked() {
        let mut f = Fixture::new(json!({}), json!({"hoverProvider": true}));
        assert_eq!(
            f.notify("$/cancelRequest", json!({"id": 999})),
            vec!["cancel/unknown-request"]
        );
        f.client_request(
            "textDocument/hover",
            json!({"textDocument": {"uri": "file:///a"}, "position": {"line": 0, "character": 0}}),
        );
        assert_eq!(
            f.server(json!({"jsonrpc": "2.0", "id": f.next_id, "error": {"code": -32800, "message": "cancelled"}})),
            vec!["cancel/cancelled-without-request"]
        );
        f.client_request(
            "textDocument/hover",
            json!({"textDocument": {"uri": "file:///a"}, "position": {"line": 0, "character": 0}}),
        );
        assert!(
            f.notify("$/cancelRequest", json!({"id": f.next_id}))
                .is_empty()
        );
        assert!(
            f.server(json!({"jsonrpc": "2.0", "id": f.next_id, "error": {"code": -32800, "message": "cancelled"}}))
                .is_empty()
        );
    }
}
