use std::fs::File;
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

use lsp_touchstone::check::{self, Outcome, Party, Report, ServerCheck};
use lsp_touchstone::finding::{Finding, Severity};
use lsp_touchstone::framing::{Frame, FrameReader, FramingError};
use lsp_touchstone::web;

const EXIT_USAGE: u8 = 252;
const EXIT_VIOLATION: u8 = 254;
const EXIT_FAILURE: u8 = 255;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(long, global = true, value_enum, default_value_t = Output::Json)]
    output: Output,
    #[arg(long, global = true)]
    debug: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum Output {
    Json,
    Text,
    Table,
}

#[derive(Subcommand)]
enum Command {
    /// Check the base protocol framing (headers and Content-Length).
    Framing {
        #[command(subcommand)]
        command: FramingCommand,
    },
    /// Check the messages between an editor and a language server.
    Proxy {
        #[command(subcommand)]
        command: ProxyCommand,
    },
    /// Act as a strict language server for an editor.
    Server {
        #[command(subcommand)]
        command: ServerCommand,
    },
    /// Drive a language server as a strict client.
    Client {
        #[command(subcommand)]
        command: ClientCommand,
    },
    /// Score a client or a server against the strict counterpart, or score a record.
    Check {
        #[command(subcommand)]
        command: CheckCommand,
    },
}

#[derive(Subcommand)]
#[allow(clippy::enum_variant_names)]
enum CheckCommand {
    /// Run a client configured to start `lsp-touchstone server run-server` as its language
    /// server, then score every session the strict server recorded. The server finds the record
    /// file through LSP_TOUCHSTONE_RECORD_FILE. The client's standard output goes to stderr.
    CheckClient {
        /// The record file to write; it is emptied first.
        #[arg(long)]
        record_file: PathBuf,
        /// Make the strict server also send the requests the client did not declare
        /// (`workspace/configuration`, `client/registerCapability`,
        /// `window/workDoneProgress/create`), to see whether the client answers every request.
        #[arg(long)]
        probe_undeclared_requests: bool,
        /// The client and its arguments; put this parameter last.
        #[arg(long, num_args = 1.., required = true, allow_hyphen_values = true)]
        client_command_args: Vec<String>,
    },
    /// Drive a language server with the strict client (initialize, open each file, ask for its
    /// symbols and a hover, shut down, exit) and score the session.
    CheckServer {
        /// The record file to write; it is emptied first.
        #[arg(long)]
        record_file: PathBuf,
        /// A file to open; repeat for more.
        #[arg(long)]
        open_file: Vec<PathBuf>,
        /// The languageId of the opened files.
        #[arg(long, default_value = "plaintext")]
        language_id: String,
        /// Seconds to wait for each round of responses and for the server to exit.
        #[arg(long, default_value_t = 30)]
        timeout_seconds: u64,
        /// Seconds to keep the session open after the requests are answered.
        #[arg(long, default_value_t = 1)]
        settle_seconds: u64,
        /// The language server and its arguments; put this parameter last.
        #[arg(long, num_args = 1.., required = true, allow_hyphen_values = true)]
        server_command_args: Vec<String>,
    },
    /// Score a record written by any mode (`--record-file`), replaying each session.
    CheckRecord {
        #[arg(long)]
        record_file: PathBuf,
        /// The side whose findings count: `client`, `server` or `both`.
        #[arg(long, value_enum, default_value_t = Target::Both)]
        target: Target,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Target {
    Client,
    Server,
    Both,
}

impl From<Target> for Party {
    fn from(target: Target) -> Party {
        match target {
            Target::Client => Party::Client,
            Target::Server => Party::Server,
            Target::Both => Party::Both,
        }
    }
}

#[derive(Subcommand)]
enum ProxyCommand {
    /// Run a language server behind a checking proxy on stdio; give this command to an editor as
    /// its language server. The web UI's URL goes to stderr.
    RunProxy {
        /// The language server and its arguments; put this parameter last.
        #[arg(long, num_args = 1.., required = true, allow_hyphen_values = true)]
        server_command_args: Vec<String>,
        /// Serve a web UI that shows the session live on this port of 127.0.0.1 (0 picks one).
        #[arg(long)]
        web_ui_port: Option<u16>,
        /// Append every message as one JSON line to this file.
        #[arg(long)]
        record_file: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ServerCommand {
    /// Run the strict server on stdio; give this command to an editor as its language server.
    /// The web UI's URL goes to stderr.
    RunServer {
        /// Serve a web UI that shows the session live on this port of 127.0.0.1 (0 picks one).
        #[arg(long)]
        web_ui_port: Option<u16>,
        /// Also send `workspace/configuration`, `client/registerCapability` and
        /// `window/workDoneProgress/create` when the client did not declare them, to see whether
        /// the client answers every request. The server's own messages then have errors.
        #[arg(long, env = "LSP_TOUCHSTONE_PROBE_UNDECLARED_REQUESTS")]
        probe_undeclared_requests: bool,
        /// Append every message as one JSON line to this file.
        #[arg(long, env = "LSP_TOUCHSTONE_RECORD_FILE")]
        record_file: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ClientCommand {
    /// Serve the web UI of the strict client and print its URL as one JSON line.
    StartWebUi {
        /// The port on 127.0.0.1; 0 picks a free port.
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Append every message as one JSON line to this file.
        #[arg(long)]
        record_file: Option<PathBuf>,
    },
    /// Print the GraphQL schema of the web UI's API.
    PrintGraphqlSchema,
}

#[derive(Subcommand)]
enum FramingCommand {
    /// Read a recorded one-direction stream and report framing violations per frame.
    CheckFrames {
        /// The recorded stream; /dev/stdin reads standard input.
        #[arg(long)]
        input_file: PathBuf,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct CheckFramesResult {
    frames: Vec<Frame>,
    fatal: Option<Finding>,
}

fn main() -> ExitCode {
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.len() > 2
        && args.last().is_some_and(|a| a == "help")
        && !args
            .iter()
            .any(|a| a == "--server-command-args" || a == "--client-command-args")
    {
        *args.last_mut().unwrap() = "--help".into();
    }
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => {
            let usage = err.use_stderr();
            let _ = err.print();
            return ExitCode::from(if usage { EXIT_USAGE } else { 0 });
        }
    };
    match run(&cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("lsp-touchstone: {err}");
            if cli.debug {
                eprintln!("{err:?}");
            }
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

fn run(cli: &Cli) -> io::Result<ExitCode> {
    match &cli.command {
        Command::Framing {
            command: FramingCommand::CheckFrames { input_file },
        } => check_frames(input_file, cli.output),
        Command::Client {
            command: ClientCommand::StartWebUi { port, record_file },
        } => start_web_ui(*port, record_file.as_deref()),
        Command::Proxy {
            command:
                ProxyCommand::RunProxy {
                    server_command_args,
                    web_ui_port,
                    record_file,
                },
        } => {
            let code = lsp_touchstone::proxy::run(lsp_touchstone::proxy::ProxyOptions {
                command: server_command_args.clone(),
                record: open_record(record_file.as_deref())?,
                web_ui_port: *web_ui_port,
            })?;
            Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
        }
        Command::Server {
            command:
                ServerCommand::RunServer {
                    web_ui_port,
                    probe_undeclared_requests,
                    record_file,
                },
        } => {
            let code = lsp_touchstone::server::run(lsp_touchstone::server::ServerOptions {
                record: open_record(record_file.as_deref())?,
                web_ui_port: *web_ui_port,
                probe_undeclared_requests: *probe_undeclared_requests,
            })?;
            Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
        }
        Command::Client {
            command: ClientCommand::PrintGraphqlSchema,
        } => {
            print!("{}", web::schema_sdl());
            Ok(ExitCode::SUCCESS)
        }
        Command::Check {
            command:
                CheckCommand::CheckClient {
                    record_file,
                    probe_undeclared_requests,
                    client_command_args,
                },
        } => print_report(
            &check::check_client(client_command_args, record_file, *probe_undeclared_requests)?,
            cli.output,
        ),
        Command::Check {
            command:
                CheckCommand::CheckServer {
                    record_file,
                    open_file,
                    language_id,
                    timeout_seconds,
                    settle_seconds,
                    server_command_args,
                },
        } => print_report(
            &check::check_server(&ServerCheck {
                command: server_command_args.clone(),
                record_file: record_file.clone(),
                open_files: open_file.clone(),
                language_id: language_id.clone(),
                timeout: Duration::from_secs(*timeout_seconds),
                settle: Duration::from_secs(*settle_seconds),
            })?,
            cli.output,
        ),
        Command::Check {
            command:
                CheckCommand::CheckRecord {
                    record_file,
                    target,
                },
        } => print_report(
            &check::score_record(record_file, (*target).into())?,
            cli.output,
        ),
    }
}

fn party_name(party: Party) -> &'static str {
    match party {
        Party::Client => "client",
        Party::Server => "server",
        Party::Both => "both",
    }
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

fn print_report(report: &Report, output: Output) -> io::Result<ExitCode> {
    let mut out = io::stdout().lock();
    match output {
        Output::Json => {
            serde_json::to_writer_pretty(&mut out, report)?;
            writeln!(out)?;
        }
        Output::Text => {
            for f in &report.findings {
                writeln!(
                    out,
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    f.session_id,
                    f.seq,
                    party_name(f.party),
                    f.severity,
                    f.code,
                    f.message
                )?;
            }
        }
        Output::Table => {
            writeln!(
                out,
                "{}: {}, {} in {} (record: {})",
                match report.target {
                    Party::Both => "client and server".to_string(),
                    party => party_name(party).to_string(),
                },
                count(report.errors, "error"),
                count(report.warnings, "warning"),
                count(report.sessions.len(), "session"),
                report.record_file
            )?;
            if report.counterpart_findings > 0 {
                writeln!(
                    out,
                    "{} on the strict counterpart, not counted",
                    count(report.counterpart_findings, "finding")
                )?;
            }
            let rows: Vec<[String; 4]> = report
                .rules
                .iter()
                .filter(|r| r.outcome != Outcome::NoFinding)
                .map(|r| {
                    let first = report
                        .findings
                        .iter()
                        .find(|f| {
                            f.code == r.code
                                && (report.target == Party::Both || f.party == report.target)
                        })
                        .map(|f| f.message.clone())
                        .unwrap_or_default();
                    [
                        r.code.clone(),
                        r.severity.clone(),
                        r.count.to_string(),
                        first,
                    ]
                })
                .collect();
            if !rows.is_empty() {
                writeln!(out)?;
                write_rows(
                    &mut out,
                    ["Code", "Severity", "Count", "First message"],
                    &rows,
                )?;
            }
        }
    }
    Ok(if report.errors > 0 {
        ExitCode::from(EXIT_VIOLATION)
    } else {
        ExitCode::SUCCESS
    })
}

fn check_frames(input_file: &PathBuf, output: Output) -> io::Result<ExitCode> {
    let mut reader = FrameReader::new(BufReader::new(File::open(input_file)?));
    let mut result = CheckFramesResult {
        frames: Vec::new(),
        fatal: None,
    };
    loop {
        match reader.read_frame() {
            Ok(Some(frame)) => result.frames.push(frame),
            Ok(None) => break,
            Err(FramingError::Fatal(finding)) => {
                result.fatal = Some(finding);
                break;
            }
            Err(FramingError::Io(err)) => return Err(err),
        }
    }

    let mut out = io::stdout().lock();
    match output {
        Output::Json => {
            serde_json::to_writer_pretty(&mut out, &result)?;
            writeln!(out)?;
        }
        Output::Text => {
            for finding in all_findings(&result) {
                writeln!(
                    out,
                    "{}\t{}\t{}\t{}",
                    finding.offset,
                    finding.severity.as_str(),
                    finding.code,
                    finding.message
                )?;
            }
        }
        Output::Table => write_table(&mut out, &all_findings(&result))?,
    }

    let violated = all_findings(&result)
        .iter()
        .any(|f| f.severity == Severity::Error);
    Ok(if violated {
        ExitCode::from(EXIT_VIOLATION)
    } else {
        ExitCode::SUCCESS
    })
}

fn all_findings(result: &CheckFramesResult) -> Vec<&Finding> {
    result
        .frames
        .iter()
        .flat_map(|frame| frame.findings.iter())
        .chain(result.fatal.iter())
        .collect()
}

fn write_table(out: &mut impl Write, findings: &[&Finding]) -> io::Result<()> {
    let rows: Vec<[String; 4]> = findings
        .iter()
        .map(|f| {
            [
                f.offset.to_string(),
                f.severity.as_str().to_string(),
                f.code.to_string(),
                f.message.clone(),
            ]
        })
        .collect();
    write_rows(out, ["Offset", "Severity", "Code", "Message"], &rows)
}

fn write_rows(out: &mut impl Write, header: [&str; 4], rows: &[[String; 4]]) -> io::Result<()> {
    let mut widths = header.map(str::len);
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let line = |cells: [&str; 4]| -> String {
        let padded: Vec<String> = cells
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        format!("| {} |", padded.join(" | ")).trim_end().to_string()
    };
    writeln!(out, "{}", line(header))?;
    let rule: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    writeln!(out, "|-{}-|", rule.join("-|-"))?;
    for row in rows {
        writeln!(out, "{}", line(row.each_ref().map(String::as_str)))?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct WebUiStarted {
    url: String,
}

fn start_web_ui(port: u16, record_file: Option<&Path>) -> io::Result<ExitCode> {
    let record = open_record(record_file)?;
    let state = Arc::new(web::AppState::new(record)?);
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let addr = listener.local_addr()?;
        let started = WebUiStarted {
            url: format!("http://127.0.0.1:{}/", addr.port()),
        };
        println!("{}", serde_json::to_string(&started)?);
        axum::serve(listener, web::router(web::schema(state), addr)).await?;
        Ok(ExitCode::SUCCESS)
    })
}

fn open_record(path: Option<&Path>) -> io::Result<Option<File>> {
    path.map(|path| File::options().create(true).append(true).open(path))
        .transpose()
}
