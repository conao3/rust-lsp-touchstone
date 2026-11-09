//! The strict server: an editor starts it as a language server, and it drives the editor through
//! the server-to-client side of the protocol while the monitor checks every message.
//!
//! What it exercises, each only when the client declared the capability:
//!
//! - progress: `window/workDoneProgress/create`, then `begin`, `report` and `end`;
//! - dynamic registration of `workspace/didChangeWatchedFiles`;
//! - `workspace/configuration` with two items, expecting two results;
//! - the negotiated position encoding, preferring `utf-32`, then `utf-8`, then `utf-16`;
//! - diagnostics on the first line and on the first non-ASCII character;
//! - hover with a range, every completion item shape (text edit, insert and replace edit,
//!   additional edits, snippet, resolve), definition as `Location` or `LocationLink`;
//! - code actions whose commands send `workspace/applyEdit` (a text edit, a file creation); the
//!   next `didChange` must produce exactly the edited text.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::document::{self, Located};
use crate::finding::{Finding, Severity};
use crate::framing::{FrameReader, FramingError};
use crate::jsonrpc;
use crate::model::capability;
use crate::monitor::{Direction, LoggedMessage, Monitor};
use crate::position::{self, Encoding};
use crate::session::write_frame;
use crate::web::{self, AppState, UiMode};

pub const APPLY_EDIT_COMMAND: &str = "lsp-touchstone.applyEdit";
pub const CREATE_FILE_COMMAND: &str = "lsp-touchstone.createFile";
const EDIT_TEXT: &str = "// edited by lsp-touchstone 😀\n";
const PROGRESS_TOKEN: &str = "lsp-touchstone-progress";

pub struct ServerOptions {
    pub record: Option<File>,
    pub web_ui_port: Option<u16>,
}

/// Serves on stdio until `exit` and returns the exit code (0 after `shutdown`, 1 otherwise).
pub fn run(options: ServerOptions) -> io::Result<i32> {
    let (events, _) = broadcast::channel(4096);
    let record = options.record.map(|f| Arc::new(Mutex::new(f)));
    let monitor = Arc::new(Monitor::new(
        1,
        vec!["lsp-touchstone".to_string(), "server".to_string()],
        false,
        events.clone(),
        record,
    ));
    if let Some(port) = options.web_ui_port {
        let state = AppState::observing(UiMode::Server, Arc::clone(&monitor), events)?;
        let url = web::serve_in_background(state, port)?;
        eprintln!("{}", json!({"Url": url}));
    }
    let code = Server::new(Arc::clone(&monitor), io::stdout()).serve(io::stdin())?;
    monitor.mark_exited(Some(code));
    Ok(code)
}

enum Purpose {
    ProgressCreate,
    Register,
    Configuration(usize),
    ApplyEdit { uri: String, expected: String },
    Other,
}

pub struct Server<W> {
    monitor: Arc<Monitor>,
    out: W,
    offset: u64,
    next_id: i64,
    pending: HashMap<String, Purpose>,
    client: Value,
    shutdown: bool,
    awaiting_edit: Option<(String, String)>,
}

impl<W: Write> Server<W> {
    pub fn new(monitor: Arc<Monitor>, out: W) -> Self {
        Server {
            monitor,
            out,
            offset: 0,
            next_id: 1,
            pending: HashMap::new(),
            client: Value::Null,
            shutdown: false,
            awaiting_edit: None,
        }
    }

    pub fn serve(&mut self, input: impl Read) -> io::Result<i32> {
        let mut reader = FrameReader::new(BufReader::new(input));
        loop {
            let frame = match reader.read_frame() {
                Ok(Some(frame)) => frame,
                Ok(None) => return Ok(1),
                Err(FramingError::Io(err)) => return Err(err),
                Err(FramingError::Fatal(finding)) => {
                    let offset = finding.offset;
                    self.monitor.observe(
                        Direction::ClientToServer,
                        String::new(),
                        offset,
                        vec![finding],
                        false,
                    );
                    return Ok(1);
                }
            };
            let body = String::from_utf8_lossy(&frame.body).into_owned();
            let logged = self.monitor.observe(
                Direction::ClientToServer,
                body,
                frame.offset,
                frame.findings,
                false,
            );
            let Ok(message) = serde_json::from_str::<Value>(&logged.body) else {
                continue;
            };
            let method = message["method"].as_str().unwrap_or_default().to_string();
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            match (message.get("id"), message.get("method")) {
                (Some(id), Some(_)) => {
                    let id = id.clone();
                    let reply = self.request(&method, &params);
                    match reply {
                        Ok(result) => {
                            self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}))?;
                        }
                        Err((code, text)) => {
                            self.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": text}}))?;
                        }
                    }
                }
                (None, Some(_)) => {
                    if method == "exit" {
                        return Ok(if self.shutdown { 0 } else { 1 });
                    }
                    self.notification(&method, &params, &logged)?;
                }
                (Some(id), None) => self.response(id, &message, &logged)?,
                (None, None) => {}
            }
        }
    }

    fn send(&mut self, message: Value) -> io::Result<LoggedMessage> {
        let body = message.to_string();
        let bytes = body.clone().into_bytes();
        let logged = self.monitor.observe(
            Direction::ServerToClient,
            body,
            self.offset,
            Vec::new(),
            false,
        );
        self.offset += write_frame(&mut self.out, &bytes)?;
        Ok(logged)
    }

    fn send_request(&mut self, method: &str, params: Value, purpose: Purpose) -> io::Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(jsonrpc::id_key(&json!(id)), purpose);
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        Ok(())
    }

    fn notify(&mut self, method: &str, params: Value) -> io::Result<()> {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))?;
        Ok(())
    }

    fn supports(&self, path: &str) -> bool {
        capability(&self.client, path).is_some_and(|v| v != &Value::Bool(false))
    }

    fn supports_value(&self, path: &str, value: &str) -> bool {
        capability(&self.client, path)
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(|v| v == value))
    }

    fn encoding(&self) -> Encoding {
        self.monitor.encoding()
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => Ok(self.initialize(params)),
            "shutdown" => {
                self.shutdown = true;
                Ok(Value::Null)
            }
            "textDocument/hover" => Ok(self.hover(params)),
            "textDocument/completion" => Ok(self.completion(params)),
            "completionItem/resolve" => {
                let mut item = params.clone();
                item["documentation"] = self.markup("Resolved by `lsp-touchstone`.");
                Ok(item)
            }
            "textDocument/definition" => Ok(self.definition(params)),
            "textDocument/codeAction" => Ok(self.code_actions(params)),
            "workspace/executeCommand" => self
                .execute_command(params)
                .map(|()| Value::Null)
                .map_err(|e| (-32603, e.to_string())),
            _ => Err((-32601, format!("lsp-touchstone does not handle {method}"))),
        }
    }

    fn initialize(&mut self, params: &Value) -> Value {
        self.client = params["capabilities"].clone();
        let encoding = ["utf-32", "utf-8", "utf-16"]
            .into_iter()
            .find(|e| *e == "utf-16" || self.supports_value("general.positionEncodings", e))
            .unwrap_or("utf-16");
        json!({
            "capabilities": {
                "positionEncoding": encoding,
                "textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": true}},
                "hoverProvider": true,
                "completionProvider": {"resolveProvider": true, "triggerCharacters": ["."]},
                "definitionProvider": true,
                "codeActionProvider": true,
                "executeCommandProvider": {"commands": [APPLY_EDIT_COMMAND, CREATE_FILE_COMMAND]},
            },
            "serverInfo": {"name": "lsp-touchstone", "version": env!("CARGO_PKG_VERSION")},
        })
    }

    fn notification(
        &mut self,
        method: &str,
        params: &Value,
        logged: &LoggedMessage,
    ) -> io::Result<()> {
        match method {
            "initialized" => self.initialized(),
            "textDocument/didOpen" | "textDocument/didChange" => {
                let Some(uri) = params["textDocument"]["uri"].as_str() else {
                    return Ok(());
                };
                let uri = uri.to_string();
                if method == "textDocument/didChange"
                    && let Some((awaited, expected)) = self.awaiting_edit.take()
                {
                    if awaited == uri {
                        self.check_edit(&uri, &expected, logged);
                    } else {
                        self.awaiting_edit = Some((awaited, expected));
                    }
                }
                self.publish_diagnostics(&uri)
            }
            "textDocument/didClose" => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    self.notify(
                        "textDocument/publishDiagnostics",
                        json!({"uri": uri, "diagnostics": []}),
                    )?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn initialized(&mut self) -> io::Result<()> {
        self.notify(
            "window/logMessage",
            json!({"type": 4, "message": "lsp-touchstone strict server is running"}),
        )?;
        if self.supports("window.showMessage") {
            self.notify(
                "window/showMessage",
                json!({"type": 3, "message": "lsp-touchstone strict server is running"}),
            )?;
        }
        if self.supports("window.workDoneProgress") {
            self.send_request(
                "window/workDoneProgress/create",
                json!({"token": PROGRESS_TOKEN}),
                Purpose::ProgressCreate,
            )?;
        }
        if self.supports("workspace.didChangeWatchedFiles.dynamicRegistration") {
            self.send_request(
                "client/registerCapability",
                json!({"registrations": [{
                    "id": "lsp-touchstone-watch",
                    "method": "workspace/didChangeWatchedFiles",
                    "registerOptions": {"watchers": [{"globPattern": "**/*.touchstone"}]},
                }]}),
                Purpose::Register,
            )?;
        }
        if self.supports("workspace.configuration") {
            self.send_request(
                "workspace/configuration",
                json!({"items": [{"section": "lsp-touchstone"}, {"section": "lsp-touchstone.second"}]}),
                Purpose::Configuration(2),
            )?;
        }
        Ok(())
    }

    fn response(&mut self, id: &Value, message: &Value, logged: &LoggedMessage) -> io::Result<()> {
        let Some(purpose) = self.pending.remove(&jsonrpc::id_key(id)) else {
            return Ok(());
        };
        let result = message.get("result");
        match purpose {
            Purpose::ProgressCreate if result.is_some() => {
                for value in [
                    json!({"kind": "begin", "title": "lsp-touchstone", "cancellable": false, "percentage": 0}),
                    json!({"kind": "report", "message": "half way", "percentage": 50}),
                    json!({"kind": "end", "message": "done"}),
                ] {
                    self.notify(
                        "$/progress",
                        json!({"token": PROGRESS_TOKEN, "value": value}),
                    )?;
                }
            }
            Purpose::Configuration(expected) => {
                if let Some(items) = result.and_then(Value::as_array)
                    && items.len() != expected
                {
                    self.monitor.add_finding(
                        logged.seq,
                        Finding::new(
                            Severity::Error,
                            "client/configuration-length-mismatch",
                            format!("workspace/configuration asked for {expected} items, but {} came back", items.len()),
                            0,
                        ),
                    );
                }
            }
            Purpose::ApplyEdit { uri, expected } => {
                if result.is_some_and(|r| r["applied"] == Value::Bool(true)) {
                    self.awaiting_edit = Some((uri, expected));
                } else {
                    let reason = result
                        .and_then(|r| r["failureReason"].as_str())
                        .unwrap_or("no failureReason");
                    self.monitor.add_finding(
                        logged.seq,
                        Finding::new(
                            Severity::Warning,
                            "client/apply-edit-refused",
                            format!("the client did not apply the edit: {reason}"),
                            0,
                        ),
                    );
                }
            }
            Purpose::ProgressCreate | Purpose::Register | Purpose::Other => {}
        }
        Ok(())
    }

    fn check_edit(&mut self, uri: &str, expected: &str, logged: &LoggedMessage) {
        let actual = self.monitor.document(uri).map(|d| d.text);
        if actual.as_deref() != Some(expected) {
            self.monitor.add_finding(
                logged.seq,
                Finding::new(
                    Severity::Error,
                    "client/apply-edit-not-reflected",
                    "the first didChange after an applied workspace/applyEdit does not produce the edited text".to_string(),
                    0,
                ),
            );
        }
    }

    fn markup(&self, text: &str) -> Value {
        if self.supports_value("textDocument.hover.contentFormat", "markdown")
            || self.supports_value(
                "textDocument.completion.completionItem.documentationFormat",
                "markdown",
            )
        {
            json!({"kind": "markdown", "value": text})
        } else {
            json!({"kind": "plaintext", "value": text.replace('`', "")})
        }
    }

    /// The line, the text of the line and the character index of a request's position.
    fn locate(&self, params: &Value) -> Option<(String, u64, String, usize)> {
        let uri = params["textDocument"]["uri"].as_str()?;
        let line = params["position"]["line"].as_u64()?;
        let character = params["position"]["character"].as_u64()?;
        let text = self.monitor.document(uri)?.text;
        let (start, end) = *document::lines(&text).get(usize::try_from(line).ok()?)?;
        let content = text[start..end].to_string();
        let byte = match document::locate(&text, self.encoding(), line, character) {
            Located::Exact(b) | Located::Clamped(b) => b - start,
            Located::SplitsChar | Located::LineOutOfRange => return None,
        };
        let index = content[..byte].chars().count();
        Some((uri.to_string(), line, content, index))
    }

    fn position(&self, line: u64, content: &str, index: usize) -> Value {
        json!({"line": line, "character": position::offset(self.encoding(), content, index)})
    }

    fn range(&self, line: u64, content: &str, start: usize, end: usize) -> Value {
        json!({"start": self.position(line, content, start), "end": self.position(line, content, end)})
    }

    fn word(content: &str, index: usize) -> (usize, usize) {
        let chars: Vec<char> = content.chars().collect();
        let is_word = |c: &char| c.is_alphanumeric() || *c == '_';
        let mut start = index.min(chars.len());
        while start > 0 && is_word(&chars[start - 1]) {
            start -= 1;
        }
        let mut end = index.min(chars.len());
        while end < chars.len() && is_word(&chars[end]) {
            end += 1;
        }
        (start, end)
    }

    fn hover(&self, params: &Value) -> Value {
        let Some((_, line, content, index)) = self.locate(params) else {
            return Value::Null;
        };
        let (start, end) = Self::word(&content, index);
        let word: String = content.chars().skip(start).take(end - start).collect();
        json!({
            "contents": self.markup(&format!("`{word}` in {:?} positions, from `lsp-touchstone`", self.encoding())),
            "range": self.range(line, &content, start, end),
        })
    }

    fn completion(&self, params: &Value) -> Value {
        let Some((_, line, content, index)) = self.locate(params) else {
            return json!({"isIncomplete": false, "items": []});
        };
        let (start, end) = Self::word(&content, index);
        let prefix = self.range(line, &content, start, index);
        let whole = self.range(line, &content, start, end);
        let mut items = vec![
            json!({"label": "touchstone_plain", "kind": 1}),
            json!({"label": "touchstone_text_edit", "textEdit": {"range": prefix, "newText": "touchstone_text_edit"}}),
            json!({"label": "touchstone_additional", "additionalTextEdits": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "// lsp-touchstone additional edit\n"}]}),
            json!({"label": "touchstone_resolve", "data": {"resolve": true}}),
            json!({"label": "touchstone_😀", "insertText": "touchstone_😀"}),
        ];
        if self.supports("textDocument.completion.completionItem.insertReplaceSupport") {
            items.push(json!({"label": "touchstone_insert_replace", "textEdit": {"newText": "touchstone_insert_replace", "insert": prefix, "replace": whole}}));
        }
        if self.supports("textDocument.completion.completionItem.snippetSupport") {
            items.push(json!({"label": "touchstone_snippet", "insertText": "touchstone_snippet(${1:arg})$0", "insertTextFormat": 2}));
        }
        json!({"isIncomplete": false, "items": items})
    }

    fn definition(&self, params: &Value) -> Value {
        let Some((uri, line, content, index)) = self.locate(params) else {
            return Value::Null;
        };
        let (start, end) = Self::word(&content, index);
        let top = json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}});
        if self.supports("textDocument.definition.linkSupport") {
            json!([{
                "originSelectionRange": self.range(line, &content, start, end),
                "targetUri": uri,
                "targetRange": top,
                "targetSelectionRange": top,
            }])
        } else {
            json!([{"uri": uri, "range": top}])
        }
    }

    fn code_actions(&self, params: &Value) -> Value {
        let Some(uri) = params["textDocument"]["uri"].as_str() else {
            return json!([]);
        };
        let mut commands = vec![json!({
            "title": "lsp-touchstone: insert a line through workspace/applyEdit",
            "command": APPLY_EDIT_COMMAND,
            "arguments": [uri],
        })];
        if self.supports_value("workspace.workspaceEdit.resourceOperations", "create") {
            commands.push(json!({
                "title": "lsp-touchstone: create a file through workspace/applyEdit",
                "command": CREATE_FILE_COMMAND,
                "arguments": [uri],
            }));
        }
        if self.supports("textDocument.codeAction.codeActionLiteralSupport") {
            Value::Array(
                commands
                    .into_iter()
                    .map(|c| json!({"title": c["title"], "kind": "", "command": c}))
                    .collect(),
            )
        } else {
            Value::Array(commands)
        }
    }

    fn execute_command(&mut self, params: &Value) -> io::Result<()> {
        let command = params["command"].as_str().unwrap_or_default();
        let Some(uri) = params["arguments"][0].as_str().map(str::to_string) else {
            return Ok(());
        };
        if !self.supports("workspace.applyEdit") {
            return self.notify(
                "window/logMessage",
                json!({"type": 2, "message": "the client does not support workspace/applyEdit"}),
            );
        }
        let document_changes = self.supports("workspace.workspaceEdit.documentChanges");
        match command {
            APPLY_EDIT_COMMAND => {
                let Some(doc) = self.monitor.document(&uri) else {
                    return Ok(());
                };
                let edit = json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": EDIT_TEXT});
                let workspace_edit = if document_changes {
                    json!({"documentChanges": [{"textDocument": {"uri": uri, "version": doc.version}, "edits": [edit]}]})
                } else {
                    json!({"changes": {uri.clone(): [edit]}})
                };
                let expected = format!("{EDIT_TEXT}{}", doc.text);
                self.send_request(
                    "workspace/applyEdit",
                    json!({"label": "lsp-touchstone edit", "edit": workspace_edit}),
                    Purpose::ApplyEdit { uri, expected },
                )
            }
            CREATE_FILE_COMMAND => {
                let created = match uri.rsplit_once('/') {
                    Some((dir, _)) => format!("{dir}/lsp-touchstone-created.txt"),
                    None => return Ok(()),
                };
                self.send_request(
                    "workspace/applyEdit",
                    json!({"label": "lsp-touchstone create", "edit": {"documentChanges": [{"kind": "create", "uri": created, "options": {"ignoreIfExists": true}}]}}),
                    Purpose::Other,
                )
            }
            _ => Ok(()),
        }
    }

    fn publish_diagnostics(&mut self, uri: &str) -> io::Result<()> {
        let Some(doc) = self.monitor.document(uri) else {
            return Ok(());
        };
        let lines = document::lines(&doc.text);
        let mut diagnostics = Vec::new();
        if let Some(&(start, end)) = lines.first() {
            let content = &doc.text[start..end];
            diagnostics.push(json!({
                "range": self.range(0, content, 0, content.chars().count()),
                "severity": 4,
                "source": "lsp-touchstone",
                "message": format!("line 1 of version {}", doc.version),
            }));
        }
        'lines: for (line, &(start, end)) in lines.iter().enumerate() {
            let content = &doc.text[start..end];
            for (index, c) in content.chars().enumerate() {
                if !c.is_ascii() {
                    diagnostics.push(json!({
                        "range": self.range(line as u64, content, index, index + 1),
                        "severity": 3,
                        "source": "lsp-touchstone",
                        "message": format!("{c:?} takes {} code units in {:?}", position::offset(self.encoding(), &c.to_string(), 1), self.encoding()),
                    }));
                    break 'lines;
                }
            }
        }
        self.notify(
            "textDocument/publishDiagnostics",
            json!({"uri": uri, "version": doc.version, "diagnostics": diagnostics}),
        )
    }
}
