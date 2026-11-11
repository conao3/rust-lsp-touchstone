//! The scoring commands end to end: the strict client scored as a client against the strict
//! server, and a record replayed to the findings that only the end of its session decides.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_lsp-touchstone");

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "lsp-touchstone-check-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> (Option<i32>, Value) {
    let output = Command::new(BIN).args(args).output().unwrap();
    let report = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "{err}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code(), report)
}

fn codes(report: &Value) -> Vec<(u64, String, String)> {
    report["Findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["Seq"].as_u64().unwrap(),
                f["Code"].as_str().unwrap().to_string(),
                f["Source"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn the_strict_client_scores_clean_as_a_client_of_the_strict_server() {
    let dir = scratch("pair");
    let client_record = dir.join("client.ndjson");
    let server_record = dir.join("server.ndjson");
    let (code, report) = run(&[
        "check",
        "check-client",
        "--record-file",
        client_record.to_str().unwrap(),
        "--client-command-args",
        BIN,
        "check",
        "check-server",
        "--record-file",
        server_record.to_str().unwrap(),
        "--open-file",
        "tests/agents/sample.tch",
        "--language-id",
        "rust",
        "--settle-seconds",
        "0",
        "--server-command-args",
        BIN,
        "server",
        "run-server",
    ]);
    assert_eq!(code, Some(0), "{report:#}");
    assert_eq!(report["Errors"], 0);
    assert_eq!(report["Warnings"], 0);
    assert_eq!(report["CommandExitCode"], 0);
    assert_eq!(report["Sessions"].as_array().unwrap().len(), 1);
    let rules = report["Rules"].as_array().unwrap();
    assert!(rules.iter().all(|r| r["Outcome"] == "no-finding"));

    let (code, again) = run(&[
        "check",
        "check-record",
        "--record-file",
        server_record.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(0), "{again:#}");
    assert!(again["Sessions"][0]["Messages"].as_u64().unwrap() >= 10);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_replayed_record_gets_the_findings_of_its_end() {
    let dir = scratch("replay");
    let record = dir.join("record.ndjson");
    let line = |seq: u64, direction: &str, body: Value| {
        json!({"SessionId": 7, "Seq": seq, "Direction": direction, "Offset": 0, "Body": body.to_string(), "Findings": [], "Auto": false}).to_string()
    };
    let lines = [
        line(
            1,
            "client-to-server",
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"processId": null, "rootUri": null, "capabilities": {}}}),
        ),
        line(
            2,
            "server-to-client",
            json!({"jsonrpc": "2.0", "id": 1, "result": {"capabilities": {}}}),
        ),
        line(
            3,
            "client-to-server",
            json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        ),
        line(
            4,
            "client-to-server",
            json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/documentSymbol", "params": {"textDocument": {"uri": "file:///a"}}}),
        ),
    ];
    fs::write(&record, lines.join("\n") + "\n").unwrap();
    let (code, report) = run(&[
        "check",
        "check-record",
        "--record-file",
        record.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(254), "{report:#}");
    assert_eq!(
        codes(&report),
        vec![
            (
                1,
                "lifecycle/shutdown-not-sent".to_string(),
                "replay".to_string()
            ),
            (
                4,
                "capability/server-did-not-declare".to_string(),
                "replay".to_string()
            ),
            (
                4,
                "jsonrpc/request-never-answered".to_string(),
                "replay".to_string()
            ),
        ]
    );
    let violated: Vec<&str> = report["Rules"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["Outcome"] == "violated")
        .map(|r| r["Code"].as_str().unwrap())
        .collect();
    assert_eq!(
        violated,
        vec![
            "jsonrpc/request-never-answered",
            "capability/server-did-not-declare"
        ]
    );

    fs::write(&record, "").unwrap();
    let output = Command::new(BIN)
        .args([
            "check",
            "check-record",
            "--record-file",
            record.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(255));
    fs::remove_dir_all(dir).unwrap();
}
