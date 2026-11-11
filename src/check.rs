//! Scoring: a client run against the strict server, a server driven by the strict client, or any
//! record, reported by rule.
//!
//! A record is replayed through a fresh monitor and ended, so a session whose server was killed
//! still gets the findings that only the end of a session decides (requests never answered, no
//! `shutdown`, no `exit`).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::jsonrpc::Kind;
use crate::monitor::{Direction, EndedBy, LoggedMessage, Monitor, SessionState};
use crate::session::{Session, SessionOptions};

/// The environment variable through which `server run-server` finds the record file when the
/// client under test starts it without `--record-file`.
pub const RECORD_FILE_ENV: &str = "LSP_TOUCHSTONE_RECORD_FILE";

/// The environment variable that makes `server run-server` send the server-to-client requests
/// the client did not declare (`--probe-undeclared-requests`).
pub const PROBE_ENV: &str = "LSP_TOUCHSTONE_PROBE_UNDECLARED_REQUESTS";

/// The side a report scores, or the side a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Party {
    Client,
    Server,
    /// Both sides: a record scored without knowing which side was under test.
    Both,
}

/// The side whose behavior a finding is about: the sender of the message, except that the side
/// that never answered a request is its receiver, and the end of a session is the client's unless
/// the server exited on its own.
fn party_of(code: &str, direction: &str) -> Party {
    let from_client = direction == "client-to-server";
    match code {
        "lifecycle/shutdown-not-sent" | "lifecycle/exit-not-sent" => Party::Client,
        "lifecycle/server-exited-without-exit" => Party::Server,
        "jsonrpc/request-never-answered" if from_client => Party::Server,
        "jsonrpc/request-never-answered" => Party::Client,
        _ if from_client => Party::Client,
        _ => Party::Server,
    }
}

const RULES: &str = include_str!("../docs/rules.md");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// At least one error.
    Violated,
    /// At least one warning.
    Warned,
    /// No finding. The rule may also not have been exercised.
    NoFinding,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RuleResult {
    pub code: String,
    pub severity: String,
    pub count: usize,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ReportedFinding {
    pub session_id: u64,
    pub seq: u64,
    pub direction: String,
    pub method: Option<String>,
    pub severity: String,
    pub code: String,
    pub message: String,
    /// The side whose behavior the finding is about.
    pub party: Party,
    /// `record` when the record has it, `replay` when only the end of the replayed session found
    /// it.
    pub source: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionSummary {
    pub session_id: u64,
    /// Who ended the session, from its `SessionEnd` line; none when the recording process was
    /// killed, which the replay counts as ended by the client.
    pub ended_by: Option<EndedBy>,
    pub messages: usize,
    pub errors: usize,
    pub warnings: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Report {
    pub record_file: String,
    /// The client or server command that was run, if any.
    pub command: Vec<String>,
    pub command_exit_code: Option<i32>,
    /// The side under test. `Errors`, `Warnings`, the sessions' counts and the rules count its
    /// findings only; the strict counterpart's go to `CounterpartFindings`.
    pub target: Party,
    pub errors: usize,
    pub warnings: usize,
    pub counterpart_findings: usize,
    pub sessions: Vec<SessionSummary>,
    pub rules: Vec<RuleResult>,
    pub findings: Vec<ReportedFinding>,
}

/// Every rule in `docs/rules.md`: its code and severity as the document states it, in the order
/// of the document.
pub fn rules() -> Vec<(String, String)> {
    RULES
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            let code = cells.get(1)?.strip_prefix('`')?.strip_suffix('`')?;
            let severity = *cells.get(2)?;
            code.contains('/')
                .then(|| (code.to_string(), severity.to_string()))
        })
        .collect()
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Default)]
struct RecordedSession {
    /// The final line of every message, by seq.
    messages: BTreeMap<u64, Value>,
    ended_by: Option<EndedBy>,
}

fn read_record(path: &Path) -> io::Result<BTreeMap<u64, RecordedSession>> {
    let mut sessions: BTreeMap<u64, RecordedSession> = BTreeMap::new();
    let reader = BufReader::new(File::open(path)?);
    for (number, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line)
            .map_err(|err| invalid(format!("{}:{}: {err}", path.display(), number + 1)))?;
        if let Some(end) = value.get("SessionEnd") {
            let (Some(session), Some(by)) = (end["SessionId"].as_u64(), end["EndedBy"].as_str())
            else {
                return Err(invalid(format!(
                    "{}:{}: a SessionEnd without SessionId and EndedBy",
                    path.display(),
                    number + 1
                )));
            };
            sessions.entry(session).or_default().ended_by = Some(if by == "server" {
                EndedBy::Server
            } else {
                EndedBy::Client
            });
            continue;
        }
        let (Some(session), Some(seq)) = (value["SessionId"].as_u64(), value["Seq"].as_u64())
        else {
            return Err(invalid(format!(
                "{}:{}: a line without SessionId and Seq",
                path.display(),
                number + 1
            )));
        };
        sessions
            .entry(session)
            .or_default()
            .messages
            .insert(seq, value);
    }
    Ok(sessions)
}

/// The messages of one session replayed through a fresh monitor that is then ended by the side
/// that ended it, or by the client when the record does not say.
fn replay(id: u64, session: &RecordedSession) -> io::Result<Vec<LoggedMessage>> {
    let messages = &session.messages;
    let (events, _) = broadcast::channel(16);
    let monitor = Monitor::new(id, Vec::new(), false, events, None);
    for (expected, (seq, message)) in (1..).zip(messages) {
        if *seq != expected {
            return Err(invalid(format!(
                "the record of session {id} has no message {expected}"
            )));
        }
        let body = message["Body"].as_str().unwrap_or_default().to_string();
        let auto = message["Auto"].as_bool().unwrap_or(false);
        let offset = message["Offset"].as_u64().unwrap_or(0);
        match message["Direction"].as_str() {
            Some("client-to-server") => {
                monitor.observe(Direction::ClientToServer, body, offset, Vec::new(), auto);
            }
            Some("server-to-client") => {
                monitor.observe(Direction::ServerToClient, body, offset, Vec::new(), auto);
            }
            _ => {
                monitor.observe_stderr(body);
            }
        }
    }
    monitor.mark_exited(None, session.ended_by.unwrap_or(EndedBy::Client));
    Ok(monitor.messages(None))
}

/// Scores every session in a record file.
pub fn score_record(path: &Path, target: Party) -> io::Result<Report> {
    let sessions = read_record(path)?;
    if sessions.is_empty() {
        return Err(invalid(format!("{} records no session", path.display())));
    }
    let mut findings = Vec::new();
    let mut summaries = Vec::new();
    let counted = |f: &ReportedFinding| target == Party::Both || f.party == target;
    for (id, session) in &sessions {
        let replayed = replay(*id, session)?;
        let start = findings.len();
        for (message, again) in session.messages.values().zip(&replayed) {
            let recorded: Vec<(String, String, String)> = message["Findings"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| {
                    let text = |key: &str| f[key].as_str().unwrap_or_default().to_string();
                    (text("Code"), text("Severity"), text("Message"))
                })
                .collect();
            let replay_only: Vec<(String, String, String)> = again
                .findings
                .iter()
                .filter(|f| !recorded.iter().any(|(code, _, _)| code == f.code))
                .map(|f| {
                    (
                        f.code.to_string(),
                        f.severity.as_str().to_string(),
                        f.message.clone(),
                    )
                })
                .collect();
            let tagged = recorded
                .into_iter()
                .map(|f| (f, "record"))
                .chain(replay_only.into_iter().map(|f| (f, "replay")));
            let direction = message["Direction"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            for ((code, severity, text), source) in tagged {
                findings.push(ReportedFinding {
                    session_id: *id,
                    seq: again.seq,
                    direction: direction.clone(),
                    method: message["Method"].as_str().map(str::to_string),
                    severity,
                    party: party_of(&code, &direction),
                    code,
                    message: text,
                    source,
                });
            }
        }
        let of_session: Vec<_> = findings[start..].iter().filter(|f| counted(f)).collect();
        summaries.push(SessionSummary {
            session_id: *id,
            ended_by: session.ended_by,
            messages: session.messages.len(),
            errors: of_session.iter().filter(|f| f.severity == "error").count(),
            warnings: of_session
                .iter()
                .filter(|f| f.severity == "warning")
                .count(),
        });
    }
    let rules = rules()
        .into_iter()
        .map(|(code, severity)| {
            let of_rule: Vec<_> = findings
                .iter()
                .filter(|f| f.code == code && counted(f))
                .collect();
            let count = of_rule.len();
            let outcome = if of_rule.iter().any(|f| f.severity == "error") {
                Outcome::Violated
            } else if count > 0 {
                Outcome::Warned
            } else {
                Outcome::NoFinding
            };
            RuleResult {
                code,
                severity,
                count,
                outcome,
            }
        })
        .collect();
    Ok(Report {
        record_file: path.display().to_string(),
        command: Vec::new(),
        command_exit_code: None,
        target,
        errors: findings
            .iter()
            .filter(|f| counted(f) && f.severity == "error")
            .count(),
        warnings: findings
            .iter()
            .filter(|f| counted(f) && f.severity == "warning")
            .count(),
        counterpart_findings: findings.iter().filter(|f| !counted(f)).count(),
        sessions: summaries,
        rules,
        findings,
    })
}

/// Creates or empties the record file and returns its absolute path.
fn fresh_record(path: &Path) -> io::Result<PathBuf> {
    File::create(path)?;
    std::fs::canonicalize(path)
}

/// Runs a client whose language server is `lsp-touchstone server run-server`, then scores the
/// sessions the strict server recorded. The client's standard output goes to standard error.
///
/// With `probe`, the strict server also sends the server-to-client requests the client did not
/// declare, to see whether the client answers every request.
pub fn check_client(command: &[String], record_file: &Path, probe: bool) -> io::Result<Report> {
    let record = fresh_record(record_file)?;
    let (program, args) = command
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the command is empty"))?;
    let status = Command::new(program)
        .args(args)
        .env(RECORD_FILE_ENV, &record)
        .env(PROBE_ENV, if probe { "true" } else { "false" })
        .stdin(Stdio::null())
        .stdout(Stdio::from(io::stderr().as_fd().try_clone_to_owned()?))
        .status()?;
    let mut report = score_record(&record, Party::Client).map_err(|err| {
        invalid(format!(
            "{err}; the client ({}) did not start lsp-touchstone server run-server, or started it without {RECORD_FILE_ENV}",
            command.join(" ")
        ))
    })?;
    report.command = command.to_vec();
    report.command_exit_code = status.code();
    Ok(report)
}

pub struct ServerCheck {
    pub command: Vec<String>,
    pub record_file: PathBuf,
    pub open_files: Vec<PathBuf>,
    pub language_id: String,
    /// How long to wait for each round of responses.
    pub timeout: Duration,
    /// How long to keep the session open after the requests are answered, for the server's own
    /// requests and notifications.
    pub settle: Duration,
}

fn client_capabilities() -> Value {
    json!({
        "general": {"positionEncodings": ["utf-8", "utf-16"]},
        "window": {"workDoneProgress": true, "showMessage": {}},
        "workspace": {
            "configuration": true,
            "applyEdit": true,
            "workspaceEdit": {"documentChanges": true},
            "didChangeWatchedFiles": {"dynamicRegistration": true},
            "workspaceFolders": true,
        },
        "textDocument": {
            "synchronization": {"didSave": true},
            "hover": {"contentFormat": ["markdown", "plaintext"]},
            "completion": {"completionItem": {"snippetSupport": true, "insertReplaceSupport": true}},
            "definition": {"linkSupport": true},
            "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
            "codeAction": {},
            "publishDiagnostics": {"versionSupport": true},
        },
    })
}

/// Waits until every request the client sent is answered, the server exits, or `timeout` passes.
fn wait_answered(session: &Session, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if session.info().state == SessionState::Exited {
            return;
        }
        let messages = session.messages(None);
        let answered = messages
            .iter()
            .filter(|m| m.direction == Direction::ClientToServer && m.kind == Some(Kind::Request))
            .all(|request| messages.iter().any(|m| m.paired_seq == Some(request.seq)));
        if answered {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn server_declares(session: &Session, initialize_seq: u64, provider: &str) -> bool {
    session
        .messages(None)
        .iter()
        .find(|m| m.paired_seq == Some(initialize_seq))
        .and_then(|m| serde_json::from_str::<Value>(&m.body).ok())
        .is_some_and(|body| {
            let value = &body["result"]["capabilities"][provider];
            !value.is_null() && value != &Value::Bool(false)
        })
}

/// Drives a language server with the strict client: initialize, open each file and ask for its
/// symbols and the hover at its start, then shut down and exit. Scores the recorded session.
pub fn check_server(options: &ServerCheck) -> io::Result<Report> {
    let record = fresh_record(&options.record_file)?;
    let file = File::options().append(true).open(&record)?;
    let (events, _keep) = broadcast::channel(4096);
    let session = Session::start(
        1,
        SessionOptions {
            command: options.command.clone(),
            auto_respond: true,
        },
        events,
        Some(Arc::new(Mutex::new(file))),
    )?;
    let root = url::Url::from_directory_path(std::env::current_dir()?).map_err(|()| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the working directory is not an absolute path",
        )
    })?;
    let root = root.as_str().trim_end_matches('/');
    let initialize = session.send_request(
        "initialize",
        Some(json!({
            "processId": std::process::id(),
            "rootUri": root,
            "workspaceFolders": [{"uri": root, "name": "root"}],
            "capabilities": client_capabilities(),
        })),
    )?;
    wait_answered(&session, options.timeout);
    if session.info().state != SessionState::Exited {
        session.send_notification("initialized", Some(json!({})))?;
        for path in &options.open_files {
            let opened = session.open_file(path, &options.language_id)?;
            let body: Value = serde_json::from_str(&opened.body)?;
            let document = json!({"uri": body["params"]["textDocument"]["uri"]});
            if server_declares(&session, initialize.seq, "documentSymbolProvider") {
                session.send_request(
                    "textDocument/documentSymbol",
                    Some(json!({"textDocument": document})),
                )?;
            }
            if server_declares(&session, initialize.seq, "hoverProvider") {
                session.send_request(
                    "textDocument/hover",
                    Some(
                        json!({"textDocument": document, "position": {"line": 0, "character": 0}}),
                    ),
                )?;
            }
        }
        wait_answered(&session, options.timeout);
        thread::sleep(options.settle);
        session.send_request("shutdown", None)?;
        wait_answered(&session, options.timeout);
        session.send_notification("exit", None)?;
        let deadline = Instant::now() + options.timeout;
        while session.info().state != SessionState::Exited && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
    }
    session.kill()?;
    let mut report = score_record(&record, Party::Server)?;
    report.command = options.command.clone();
    report.command_exit_code = session.info().exit_code;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_is_read_from_the_document() {
        let rules = rules();
        assert_eq!(
            rules.len(),
            RULES.lines().filter(|l| l.starts_with("| `")).count()
        );
        assert!(rules.contains(&("lifecycle/exit-not-sent".to_string(), "warning".to_string())));
        assert!(
            rules
                .iter()
                .any(|(code, _)| code == "capability/server-did-not-declare")
        );
    }
}
