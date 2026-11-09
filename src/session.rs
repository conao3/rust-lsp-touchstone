//! A strict client session: one language server process driven over stdio. Every message in
//! both directions goes through a [`Monitor`].

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::framing::{FrameReader, FramingError};
use crate::jsonrpc::Kind;
use crate::monitor::Monitor;
pub use crate::monitor::{Direction, Event, LoggedMessage, SessionInfo, SessionState};

pub struct SessionOptions {
    pub command: Vec<String>,
    pub auto_respond: bool,
}

struct Writer {
    stdin: Option<ChildStdin>,
    offset: u64,
    next_request_id: i64,
}

pub struct Session {
    monitor: Arc<Monitor>,
    auto_respond: bool,
    child: Mutex<Child>,
    writer: Mutex<Writer>,
}

/// Spawns `command` with piped stdio.
pub fn spawn(command: &[String]) -> io::Result<Child> {
    let (program, args) = command
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the command is empty"))?;
    Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

/// Logs each line of a server's stderr; `echo` also copies it to this process's stderr.
pub fn read_stderr(monitor: &Monitor, stderr: impl Read, echo: bool) {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                if echo {
                    let _ = io::stderr().write_all(&line);
                }
                let text = String::from_utf8_lossy(&line)
                    .trim_end_matches(['\r', '\n'])
                    .to_string();
                monitor.observe_stderr(text);
            }
        }
    }
}

pub fn write_frame(out: &mut impl Write, body: &[u8]) -> io::Result<u64> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    out.write_all(header.as_bytes())?;
    out.write_all(body)?;
    out.flush()?;
    Ok((header.len() + body.len()) as u64)
}

impl Session {
    pub fn start(
        id: u64,
        options: SessionOptions,
        events: broadcast::Sender<Event>,
        record: Option<Arc<Mutex<File>>>,
    ) -> io::Result<Arc<Session>> {
        let mut child = spawn(&options.command)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let monitor = Arc::new(Monitor::new(
            id,
            options.command,
            options.auto_respond,
            events,
            record,
        ));
        let session = Arc::new(Session {
            monitor,
            auto_respond: options.auto_respond,
            child: Mutex::new(child),
            writer: Mutex::new(Writer {
                stdin,
                offset: 0,
                next_request_id: 1,
            }),
        });

        let reader = Arc::clone(&session);
        thread::spawn(move || {
            reader.read_server(stdout);
            reader.wait_for_exit();
        });
        let stderr_monitor = Arc::clone(&session.monitor);
        thread::spawn(move || read_stderr(&stderr_monitor, stderr, false));
        Ok(session)
    }

    pub fn monitor(&self) -> &Arc<Monitor> {
        &self.monitor
    }

    pub fn info(&self) -> SessionInfo {
        self.monitor.info()
    }

    pub fn messages(&self, after_seq: Option<u64>) -> Vec<LoggedMessage> {
        self.monitor.messages(after_seq)
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
        self.exited(status);
        Ok(())
    }

    fn exited(&self, status: ExitStatus) {
        self.writer.lock().unwrap().stdin = None;
        self.monitor.mark_exited(status.code());
    }

    pub fn send_request(&self, method: &str, params: Option<Value>) -> io::Result<LoggedMessage> {
        let mut writer = self.writer.lock().unwrap();
        let id = writer.next_request_id;
        writer.next_request_id += 1;
        let mut message = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if let Some(params) = params {
            message["params"] = params;
        }
        self.send_locked(&mut writer, message.to_string(), false)
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
        let mut writer = self.writer.lock().unwrap();
        self.send_locked(&mut writer, message.to_string(), false)
    }

    /// Sends `body` as the content of one frame, unchanged, even when it is not valid JSON-RPC.
    pub fn send_raw(&self, body: String) -> io::Result<LoggedMessage> {
        let mut writer = self.writer.lock().unwrap();
        self.send_locked(&mut writer, body, false)
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
        writer: &mut Writer,
        body: String,
        auto: bool,
    ) -> io::Result<LoggedMessage> {
        let stdin = writer.stdin.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "the server's stdin is closed")
        })?;
        let written = match write_frame(stdin, body.as_bytes()) {
            Ok(n) => n,
            Err(err) => {
                writer.stdin = None;
                return Err(err);
            }
        };
        let offset = writer.offset;
        writer.offset += written;
        Ok(self
            .monitor
            .observe(Direction::ClientToServer, body, offset, Vec::new(), auto))
    }

    fn read_server(&self, stdout: impl Read) {
        let mut reader = FrameReader::new(BufReader::new(stdout));
        loop {
            match reader.read_frame() {
                Ok(Some(frame)) => {
                    let body = String::from_utf8_lossy(&frame.body).into_owned();
                    let logged = self.monitor.observe(
                        Direction::ServerToClient,
                        body,
                        frame.offset,
                        frame.findings,
                        false,
                    );
                    if self.auto_respond && logged.kind == Some(Kind::Request) {
                        self.auto_respond(&logged);
                    }
                }
                Ok(None) | Err(FramingError::Io(_)) => return,
                Err(FramingError::Fatal(finding)) => {
                    let offset = finding.offset;
                    self.monitor.observe(
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

    fn wait_for_exit(&self) {
        loop {
            let status = self.child.lock().unwrap().try_wait();
            match status {
                Ok(Some(status)) => {
                    self.exited(status);
                    return;
                }
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(_) => return,
            }
        }
    }

    fn auto_respond(&self, request: &LoggedMessage) {
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
        let mut writer = self.writer.lock().unwrap();
        let _ = self.send_locked(&mut writer, response.to_string(), true);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
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
        let early = session
            .send_notification("initialized", Some(json!({})))
            .unwrap();
        assert_eq!(
            codes(&early),
            vec!["lifecycle/client-message-before-initialize-result"]
        );
        drop(session);

        let (session, _events) = scripted_server(&frame(response));
        let request = session
            .send_request(
                "initialize",
                Some(json!({"processId": null, "rootUri": null, "capabilities": {}})),
            )
            .unwrap();
        assert!(request.findings.is_empty());
        let messages = wait_for(&session, 2);
        assert_eq!(messages[1].paired_seq, Some(request.seq));
        assert!(messages[1].findings.is_empty());

        session.send_request("shutdown", None).unwrap();
        let late = session
            .send_notification("initialized", Some(json!({})))
            .unwrap();
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
