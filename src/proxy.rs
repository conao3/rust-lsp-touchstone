//! The checking proxy: an editor starts it as its language server, and it starts the real server,
//! forwards every frame in both directions and checks every message on the way.
//!
//! Frames are forwarded with their content unchanged and a fresh `Content-Length` header, so a
//! malformed header from one side reaches the other side well formed (and is reported).

use std::fs::File;
use std::io::{self, BufReader};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tokio::sync::broadcast;

use crate::framing::{FrameReader, FramingError};
use crate::monitor::{Direction, Monitor};
use crate::session::{read_stderr, spawn, write_frame};
use crate::web::{self, AppState, UiMode};

pub struct ProxyOptions {
    pub command: Vec<String>,
    pub record: Option<File>,
    pub web_ui_port: Option<u16>,
}

/// Runs until the server exits and returns its exit code.
pub fn run(options: ProxyOptions) -> io::Result<i32> {
    let (events, _) = broadcast::channel(4096);
    let record = options.record.map(|f| Arc::new(Mutex::new(f)));
    let monitor = Arc::new(Monitor::new(
        1,
        options.command.clone(),
        false,
        events.clone(),
        record,
    ));
    let mut child = spawn(&options.command)?;
    let mut server_stdin = child.stdin.take().expect("stdin is piped");
    let server_stdout = child.stdout.take().expect("stdout is piped");
    let server_stderr = child.stderr.take().expect("stderr is piped");

    if let Some(port) = options.web_ui_port {
        let state = AppState::observing(UiMode::Proxy, Arc::clone(&monitor), events)?;
        let url = web::serve_in_background(state, port)?;
        eprintln!("{}", json!({"Url": url}));
    }

    let to_editor = {
        let monitor = Arc::clone(&monitor);
        thread::spawn(move || {
            forward(
                &monitor,
                server_stdout,
                &mut io::stdout(),
                Direction::ServerToClient,
            )
        })
    };
    {
        let monitor = Arc::clone(&monitor);
        thread::spawn(move || read_stderr(&monitor, server_stderr, true));
    }
    {
        let monitor = Arc::clone(&monitor);
        thread::spawn(move || {
            forward(
                &monitor,
                io::stdin(),
                &mut server_stdin,
                Direction::ClientToServer,
            );
        });
    }

    let status = child.wait()?;
    let _ = to_editor.join();
    monitor.mark_exited(status.code());
    Ok(status.code().unwrap_or(1))
}

fn forward(monitor: &Monitor, from: impl io::Read, to: &mut impl io::Write, direction: Direction) {
    let mut reader = FrameReader::new(BufReader::new(from));
    loop {
        match reader.read_frame() {
            Ok(Some(frame)) => {
                let written = write_frame(to, &frame.body);
                let body = String::from_utf8_lossy(&frame.body).into_owned();
                monitor.observe(direction, body, frame.offset, frame.findings, false);
                if written.is_err() {
                    return;
                }
            }
            Ok(None) | Err(FramingError::Io(_)) => return,
            Err(FramingError::Fatal(finding)) => {
                let offset = finding.offset;
                monitor.observe(direction, String::new(), offset, vec![finding], false);
                return;
            }
        }
    }
}
