use std::fs::File;
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

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
    /// Drive a language server as a strict client.
    Client {
        #[command(subcommand)]
        command: ClientCommand,
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
    let cli = match Cli::try_parse() {
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
        Command::Client {
            command: ClientCommand::PrintGraphqlSchema,
        } => {
            print!("{}", web::schema_sdl());
            Ok(ExitCode::SUCCESS)
        }
    }
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
    let header = ["Offset", "Severity", "Code", "Message"];
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
    let mut widths = header.map(str::len);
    for row in &rows {
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
    for row in &rows {
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
    let record = record_file
        .map(|path| File::options().create(true).append(true).open(path))
        .transpose()?;
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
