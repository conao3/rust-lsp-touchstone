//! A strict client session: one language server process driven over stdio, with every message
//! in both directions logged and checked.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::finding::{Finding, Severity};
use crate::framing::{FrameReader, FramingError};
use crate::jsonrpc::{self, Kind};

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

pub struct SessionOptions {
    pub command: Vec<String>,
    pub auto_respond: bool,
}

const NOTIFICATIONS_BEFORE_INITIALIZE_RESULT: &[&str] = &[
    "window/showMessage",
    "window/logMessage",
    "telemetry/event",
    "$/progress",
];

struct Pending {
    seq: u64,
    method: String,
}

struct Inner {
    log: Vec<LoggedMessage>,
    next_request_id: i64,
    client_offset: u64,
    pending_client: HashMap<String, Pending>,
    pending_server: HashMap<String, Pending>,
    initialize_answered: bool,
    shutdown_sent: bool,
    state: SessionState,
    exit_code: Option<i32>,
    stdin: Option<ChildStdin>,
}

pub struct Session {
    id: u64,
    command: Vec<String>,
    auto_respond: bool,
    started: Instant,
    inner: Mutex<Inner>,
    child: Mutex<Child>,
    events: broadcast::Sender<Event>,
    record: Option<Arc<Mutex<File>>>,
}

impl Session {
    pub fn start(
        id: u64,
        options: SessionOptions,
        events: broadcast::Sender<Event>,
        record: Option<Arc<Mutex<File>>>,
    ) -> io::Result<Arc<Session>> {
        let (program, args) = options
            .command
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the command is empty"))?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");

        let session = Arc::new(Session {
            id,
            command: options.command,
            auto_respond: options.auto_respond,
            started: Instant::now(),
            inner: Mutex::new(Inner {
                log: Vec::new(),
                next_request_id: 1,
                client_offset: 0,
                pending_client: HashMap::new(),
                pending_server: HashMap::new(),
                initialize_answered: false,
                shutdown_sent: false,
                state: SessionState::Running,
                exit_code: None,
                stdin,
            }),
            child: Mutex::new(child),
            events,
            record,
        });
        let _ = session.events.send(Event::Session(session.info()));

        let reader = Arc::clone(&session);
        thread::spawn(move || {
            reader.read_server(stdout);
            reader.wait_for_exit();
        });
        let stderr_reader = Arc::clone(&session);
        thread::spawn(move || stderr_reader.read_stderr(stderr));
        Ok(session)
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn info(&self) -> SessionInfo {
        let inner = self.inner.lock().unwrap();
        SessionInfo {
            id: self.id,
            command: self.command.clone(),
            state: inner.state,
            exit_code: inner.exit_code,
            auto_respond: self.auto_respond,
        }
    }

    pub fn messages(&self, after_seq: Option<u64>) -> Vec<LoggedMessage> {
        let inner = self.inner.lock().unwrap();
        inner
            .log
            .iter()
            .filter(|m| after_seq.is_none_or(|after| m.seq > after))
            .cloned()
            .collect()
    }

    /// Kills the server and waits for it, so that the session is exited when this returns.
    pub fn kill(&self) -> io::Result<()> {
        let status = {
            let mut child = self.child.lock().unwrap();
            match child.try_wait()? {
                Some(status) => status,
                None => {
                    child.kill()?;
                    child.wait()?
                }
            }
        };
        self.mark_exited(status);
        Ok(())
    }

    fn mark_exited(&self, status: ExitStatus) {
        let mut inner = self.inner.lock().unwrap();
        if inner.state == SessionState::Exited {
            return;
        }
        inner.state = SessionState::Exited;
        inner.exit_code = status.code();
        inner.stdin = None;
        drop(inner);
        let _ = self.events.send(Event::Session(self.info()));
    }

    pub fn send_request(&self, method: &str, params: Option<Value>) -> io::Result<LoggedMessage> {
        let mut inner = self.inner.lock().unwrap();
        let id = inner.next_request_id;
        inner.next_request_id += 1;
        let mut message = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if let Some(params) = params {
            message["params"] = params;
        }
        self.send_locked(&mut inner, message.to_string(), false)
    }

    pub fn send_notification(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> io::Result<LoggedMessage> {
        let mut message = json!({"jsonrpc": "2.0", "method": method});
        if let Some(params) = params {
            message["params"] = params;
        }
        let mut inner = self.inner.lock().unwrap();
        self.send_locked(&mut inner, message.to_string(), false)
    }

    /// Sends `body` as the content of one frame, unchanged, even when it is not valid JSON-RPC.
    pub fn send_raw(&self, body: String) -> io::Result<LoggedMessage> {
        let mut inner = self.inner.lock().unwrap();
        self.send_locked(&mut inner, body, false)
    }

    pub fn open_file(&self, path: &Path, language_id: &str) -> io::Result<LoggedMessage> {
        let path = std::fs::canonicalize(path)?;
        let text = std::fs::read_to_string(&path)?;
        let uri = url::Url::from_file_path(&path)
            .map_err(|()| io::Error::new(io::ErrorKind::InvalidInput, "not an absolute path"))?;
        self.send_notification(
            "textDocument/didOpen",
            Some(json!({
                "textDocument": {
                    "uri": uri.as_str(),
                    "languageId": language_id,
                    "version": 1,
                    "text": text,
                }
            })),
        )
    }

    fn send_locked(
        &self,
        inner: &mut Inner,
        body: String,
        auto: bool,
    ) -> io::Result<LoggedMessage> {
        let stdin = inner.stdin.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "the server's stdin is closed")
        })?;
        let frame = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        if let Err(err) = stdin
            .write_all(frame.as_bytes())
            .and_then(|()| stdin.flush())
        {
            inner.stdin = None;
            return Err(err);
        }
        let offset = inner.client_offset;
        inner.client_offset += frame.len() as u64;
        Ok(self.log_locked(
            inner,
            Direction::ClientToServer,
            body,
            offset,
            Vec::new(),
            auto,
        ))
    }

    fn read_server(self: &Arc<Self>, stdout: impl io::Read) {
        let mut reader = FrameReader::new(BufReader::new(stdout));
        loop {
            match reader.read_frame() {
                Ok(Some(frame)) => {
                    let body = String::from_utf8_lossy(&frame.body).into_owned();
                    let mut inner = self.inner.lock().unwrap();
                    let logged = self.log_locked(
                        &mut inner,
                        Direction::ServerToClient,
                        body,
                        frame.offset,
                        frame.findings,
                        false,
                    );
                    if self.auto_respond && logged.kind == Some(Kind::Request) {
                        self.auto_respond_locked(&mut inner, &logged);
                    }
                }
                Ok(None) | Err(FramingError::Io(_)) => return,
                Err(FramingError::Fatal(finding)) => {
                    let mut inner = self.inner.lock().unwrap();
                    let offset = finding.offset;
                    self.log_locked(
                        &mut inner,
                        Direction::ServerToClient,
                        String::new(),
                        offset,
                        vec![finding],
                        false,
                    );
                    return;
                }
            }
        }
    }

    fn read_stderr(&self, stderr: impl io::Read) {
        let mut reader = BufReader::new(stderr);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&line)
                        .trim_end_matches(['\r', '\n'])
                        .to_string();
                    let mut inner = self.inner.lock().unwrap();
                    let seq = inner.log.len() as u64 + 1;
                    let message = LoggedMessage {
                        session_id: self.id,
                        seq,
                        direction: Direction::ServerStderr,
                        time_ms: self.elapsed_ms(),
                        kind: None,
                        method: None,
                        id: None,
                        body: text,
                        findings: Vec::new(),
                        paired_seq: None,
                        auto: false,
                    };
                    self.publish_locked(&mut inner, message);
                }
            }
        }
    }

    fn wait_for_exit(&self) {
        loop {
            let status = self.child.lock().unwrap().try_wait();
            match status {
                Ok(Some(status)) => {
                    self.mark_exited(status);
                    return;
                }
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(_) => return,
            }
        }
    }

    fn auto_respond_locked(&self, inner: &mut Inner, request: &LoggedMessage) {
        let Ok(value) = serde_json::from_str::<Value>(&request.body) else {
            return;
        };
        let Some(id) = value.get("id").cloned() else {
            return;
        };
        let method = request.method.as_deref().unwrap_or_default();
        let outcome = match method {
            "workspace/configuration" => {
                let items = value["params"]["items"]
                    .as_array()
                    .map_or(0, |items| items.len());
                Ok(Value::Array(vec![Value::Null; items]))
            }
            "workspace/applyEdit" => Ok(json!({
                "applied": false,
                "failureReason": "lsp-touchstone does not apply workspace edits",
            })),
            "window/showDocument" => Ok(json!({"success": false})),
            "window/workDoneProgress/create"
            | "client/registerCapability"
            | "client/unregisterCapability"
            | "window/showMessageRequest"
            | "workspace/workspaceFolders" => Ok(Value::Null),
            m if m.starts_with("workspace/") && m.ends_with("/refresh") => Ok(Value::Null),
            _ => Err(
                json!({"code": -32601, "message": format!("lsp-touchstone does not handle {method}")}),
            ),
        };
        let response = match outcome {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        };
        let _ = self.send_locked(inner, response.to_string(), true);
    }

    fn elapsed_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }

    fn log_locked(
        &self,
        inner: &mut Inner,
        direction: Direction,
        body: String,
        offset: u64,
        mut findings: Vec<Finding>,
        auto: bool,
    ) -> LoggedMessage {
        let seq = inner.log.len() as u64 + 1;
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
                    paired_seq =
                        check_session(inner, direction, &classified, seq, offset, &mut findings);
                    kind = Some(classified.kind);
                    method = classified.method.or_else(|| {
                        paired_seq.and_then(|p| {
                            inner.log.get(p as usize - 1).and_then(|m| m.method.clone())
                        })
                    });
                    id = classified.id.as_ref().map(jsonrpc::id_key);
                }
            }
        }

        let message = LoggedMessage {
            session_id: self.id,
            seq,
            direction,
            time_ms: self.elapsed_ms(),
            kind,
            method,
            id,
            body,
            findings,
            paired_seq,
            auto,
        };
        self.publish_locked(inner, message.clone());
        message
    }

    fn publish_locked(&self, inner: &mut Inner, message: LoggedMessage) {
        if let Some(record) = &self.record
            && let Ok(line) = serde_json::to_string(&message)
        {
            let mut file = record.lock().unwrap();
            let _ = writeln!(file, "{line}");
        }
        inner.log.push(message.clone());
        let _ = self.events.send(Event::Message(Box::new(message)));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

/// Pairs requests with responses and checks the lifecycle; returns the paired request's seq.
fn check_session(
    inner: &mut Inner,
    direction: Direction,
    message: &jsonrpc::Classified,
    seq: u64,
    offset: u64,
    findings: &mut Vec<Finding>,
) -> Option<u64> {
    let method = message.method.as_deref().unwrap_or_default();
    let key = message.id.as_ref().map(jsonrpc::id_key);
    let mut error = |code: &'static str, text: String| {
        findings.push(Finding::new(Severity::Error, code, text, offset));
    };
    let from_client = direction == Direction::ClientToServer;

    if from_client {
        if inner.shutdown_sent && !(message.kind == Kind::Notification && method == "exit") {
            error(
                "lifecycle/client-message-after-shutdown",
                "after a shutdown request the client may only send the exit notification"
                    .to_string(),
            );
        } else if !inner.initialize_answered
            && !(message.kind == Kind::Request && method == "initialize")
            && !(message.kind == Kind::Notification && method == "exit")
        {
            error(
                "lifecycle/client-message-before-initialize-result",
                "the client must not send requests or notifications before the server answers initialize".to_string(),
            );
        }
    } else if !inner.initialize_answered {
        let allowed = match message.kind {
            Kind::Notification => NOTIFICATIONS_BEFORE_INITIALIZE_RESULT.contains(&method),
            Kind::Request => method == "window/showMessageRequest",
            Kind::Response | Kind::Invalid => true,
        };
        if !allowed {
            error(
                "lifecycle/server-message-before-initialize-result",
                format!("the server must not send {method} before it answers initialize"),
            );
        }
    }

    let (own, other) = if from_client {
        (&mut inner.pending_client, &mut inner.pending_server)
    } else {
        (&mut inner.pending_server, &mut inner.pending_client)
    };
    let key = key?;
    match message.kind {
        Kind::Request => {
            if own.contains_key(&key) {
                error(
                    "jsonrpc/duplicate-request-id",
                    format!("the id {key} is already used by a request without a response"),
                );
            }
            own.insert(
                key,
                Pending {
                    seq,
                    method: method.to_string(),
                },
            );
            if from_client && method == "shutdown" {
                inner.shutdown_sent = true;
            }
            None
        }
        Kind::Response => match other.remove(&key) {
            Some(pending) => {
                if !from_client && pending.method == "initialize" {
                    inner.initialize_answered = true;
                }
                Some(pending.seq)
            }
            None => {
                error(
                    "jsonrpc/response-to-unknown-request",
                    format!("no request with the id {key} is waiting for a response"),
                );
                None
            }
        },
        Kind::Notification | Kind::Invalid => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(body: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
    }

    /// Starts `sh` as a server that writes `output` and then reads its stdin until it closes.
    fn scripted_server(output: &str) -> (Arc<Session>, broadcast::Receiver<Event>) {
        let (events, receiver) = broadcast::channel(64);
        let script = format!("printf '%s' '{output}'; cat > /dev/null");
        let session = Session::start(
            1,
            SessionOptions {
                command: vec!["sh".to_string(), "-c".to_string(), script],
                auto_respond: true,
            },
            events,
            None,
        )
        .unwrap();
        (session, receiver)
    }

    fn wait_for(session: &Session, count: usize) -> Vec<LoggedMessage> {
        for _ in 0..200 {
            let messages = session.messages(None);
            if messages.len() >= count {
                return messages;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "expected {count} messages, got {:?}",
            session.messages(None)
        );
    }

    fn codes(message: &LoggedMessage) -> Vec<&'static str> {
        message.findings.iter().map(|f| f.code).collect()
    }

    #[test]
    fn answers_a_server_request_and_reports_it_before_initialize() {
        let request = r#"{"jsonrpc":"2.0","id":"s1","method":"workspace/configuration","params":{"items":[{},{}]}}"#;
        let (session, _events) = scripted_server(&frame(request));
        let messages = wait_for(&session, 2);

        assert_eq!(messages[0].direction, Direction::ServerToClient);
        assert_eq!(
            codes(&messages[0]),
            vec!["lifecycle/server-message-before-initialize-result"]
        );

        let answer = &messages[1];
        assert!(answer.auto);
        assert_eq!(answer.paired_seq, Some(1));
        assert_eq!(answer.method.as_deref(), Some("workspace/configuration"));
        let body: Value = serde_json::from_str(&answer.body).unwrap();
        assert_eq!(body["result"], json!([null, null]));
        session.kill().unwrap();
    }

    #[test]
    fn pairs_responses_and_checks_the_lifecycle_of_the_client() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#;
        let (session, _events) = scripted_server("");
        let early = session.send_notification("initialized", None).unwrap();
        assert_eq!(
            codes(&early),
            vec!["lifecycle/client-message-before-initialize-result"]
        );
        drop(session);

        let (session, _events) = scripted_server(&frame(response));
        let request = session
            .send_request("initialize", Some(json!({"capabilities": {}})))
            .unwrap();
        assert!(request.findings.is_empty());
        let messages = wait_for(&session, 2);
        assert_eq!(messages[1].paired_seq, Some(request.seq));
        assert!(messages[1].findings.is_empty());

        session.send_request("shutdown", None).unwrap();
        let late = session.send_notification("initialized", None).unwrap();
        assert_eq!(
            codes(&late),
            vec!["lifecycle/client-message-after-shutdown"]
        );
        let exit = session.send_notification("exit", None).unwrap();
        assert!(exit.findings.is_empty());
        session.kill().unwrap();
        assert_eq!(session.info().state, SessionState::Exited);
    }
}
