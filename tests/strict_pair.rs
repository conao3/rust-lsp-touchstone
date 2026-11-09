//! The strict client against the strict server: every message both of them send must pass every
//! check, and the server's scenario (progress, registration, configuration, edits) must run.

use std::thread;
use std::time::Duration;

use lsp_touchstone::finding::Severity;
use lsp_touchstone::session::{LoggedMessage, Session, SessionOptions};
use serde_json::{Value, json};
use tokio::sync::broadcast;

fn wait_until(session: &Session, what: &str, done: impl Fn(&[LoggedMessage]) -> bool) {
    for _ in 0..500 {
        if done(&session.messages(None)) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "timed out waiting for {what}: {:#?}",
        session.messages(None)
    );
}

fn has(messages: &[LoggedMessage], method: &str) -> bool {
    messages.iter().any(|m| m.method.as_deref() == Some(method))
}

fn body(message: &LoggedMessage) -> Value {
    serde_json::from_str(&message.body).unwrap()
}

#[test]
fn the_strict_client_and_the_strict_server_agree() {
    let (events, _keep) = broadcast::channel(1024);
    let session = Session::start(
        1,
        SessionOptions {
            command: vec![
                env!("CARGO_BIN_EXE_lsp-touchstone").to_string(),
                "server".to_string(),
                "run-server".to_string(),
            ],
            auto_respond: true,
        },
        events,
        None,
    )
    .unwrap();

    let uri = "file:///tmp/lsp-touchstone-test/a.rs";
    session
        .send_request(
            "initialize",
            Some(json!({
                "processId": null,
                "rootUri": null,
                "capabilities": {
                    "general": {"positionEncodings": ["utf-8", "utf-16"]},
                    "window": {"workDoneProgress": true, "showMessage": {}},
                    "workspace": {
                        "configuration": true,
                        "applyEdit": true,
                        "workspaceEdit": {"documentChanges": true},
                        "didChangeWatchedFiles": {"dynamicRegistration": true},
                    },
                    "textDocument": {
                        "synchronization": {"didSave": true},
                        "hover": {"contentFormat": ["markdown"]},
                        "completion": {"completionItem": {"snippetSupport": true, "insertReplaceSupport": true}},
                        "definition": {"linkSupport": true},
                        "codeAction": {},
                        "publishDiagnostics": {"versionSupport": true},
                    },
                },
            })),
        )
        .unwrap();
    wait_until(&session, "the initialize result", |m| m.len() >= 2);
    let result = body(&session.messages(None)[1]);
    assert_eq!(
        result["result"]["capabilities"]["positionEncoding"],
        "utf-8"
    );

    session
        .send_notification("initialized", Some(json!({})))
        .unwrap();
    let text = "fn main() {}\nlet s = \"日本😀\";\n";
    session
        .send_notification(
            "textDocument/didOpen",
            Some(json!({"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": text}})),
        )
        .unwrap();
    let at = |line: u32, character: u32| json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": character}});
    session
        .send_request("textDocument/hover", Some(at(0, 4)))
        .unwrap();
    session
        .send_request("textDocument/completion", Some(at(0, 4)))
        .unwrap();
    session
        .send_request("textDocument/definition", Some(at(0, 4)))
        .unwrap();
    session
        .send_request(
            "workspace/executeCommand",
            Some(json!({"command": "lsp-touchstone.applyEdit", "arguments": [uri]})),
        )
        .unwrap();
    session
        .send_notification(
            "textDocument/didChange",
            Some(json!({"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"range": {"start": {"line": 1, "character": 9}, "end": {"line": 1, "character": 15}}, "text": "x"}]})),
        )
        .unwrap();

    wait_until(&session, "the scenario", |m| {
        has(m, "$/progress")
            && has(m, "client/registerCapability")
            && has(m, "workspace/configuration")
            && has(m, "workspace/applyEdit")
            && m.iter()
                .filter(|m| m.method.as_deref() == Some("textDocument/publishDiagnostics"))
                .count()
                >= 2
    });
    session.send_request("shutdown", None).unwrap();
    session.send_notification("exit", None).unwrap();
    wait_until(&session, "the exit", |_| {
        session.info().state == lsp_touchstone::session::SessionState::Exited
    });

    let messages = session.messages(None);
    let errors: Vec<_> = messages
        .iter()
        .flat_map(|m| m.findings.iter().map(move |f| (m.seq, m.method.clone(), f)))
        .filter(|(_, _, f)| f.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(session.info().exit_code, Some(0));

    let diagnostics: Vec<Value> = messages
        .iter()
        .filter(|m| m.method.as_deref() == Some("textDocument/publishDiagnostics"))
        .map(|m| body(m)["params"].clone())
        .collect();
    let last = diagnostics.last().unwrap();
    assert_eq!(last["version"], 2);
    let non_ascii = &last["diagnostics"][1]["range"];
    assert_eq!(non_ascii["start"], json!({"line": 1, "character": 10}));
    assert_eq!(non_ascii["end"], json!({"line": 1, "character": 14}));
}
