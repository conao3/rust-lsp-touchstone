//! The base protocol framing: a header part of `Name: value\r\n` fields ended by `\r\n`,
//! followed by a body of exactly `Content-Length` bytes.

use std::io::{self, BufRead};

use serde::Serialize;

use crate::finding::{Finding, Severity};

const DEFAULT_MIME: &str = "application/vscode-jsonrpc";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Frame {
    pub offset: u64,
    pub content_length: usize,
    #[serde(skip)]
    pub body: Vec<u8>,
    pub findings: Vec<Finding>,
}

#[derive(Debug)]
pub enum FramingError {
    Io(io::Error),
    Fatal(Finding),
}

impl From<io::Error> for FramingError {
    fn from(err: io::Error) -> Self {
        FramingError::Io(err)
    }
}

pub struct FrameReader<R> {
    inner: R,
    offset: u64,
}

impl<R: BufRead> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        FrameReader { inner, offset: 0 }
    }

    pub fn read_frame(&mut self) -> Result<Option<Frame>, FramingError> {
        let start = self.offset;
        let mut findings = Vec::new();
        let mut content_length: Option<usize> = None;
        let mut content_type_seen = false;
        let mut first = true;

        loop {
            let line_offset = self.offset;
            let mut line = Vec::new();
            let n = self.inner.read_until(b'\n', &mut line)?;
            self.offset += n as u64;
            if n == 0 {
                if first {
                    return Ok(None);
                }
                return Err(FramingError::Fatal(Finding::new(
                    Severity::Error,
                    "framing/unexpected-eof-in-header",
                    "the stream ended inside a header part",
                    line_offset,
                )));
            }
            first = false;

            if line.last() != Some(&b'\n') {
                return Err(FramingError::Fatal(Finding::new(
                    Severity::Error,
                    "framing/unexpected-eof-in-header",
                    "the stream ended inside a header field",
                    line_offset,
                )));
            }
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            } else {
                findings.push(Finding::new(
                    Severity::Error,
                    "framing/header-line-terminator",
                    "a header line must end with \\r\\n, not a bare \\n",
                    line_offset,
                ));
            }

            if line.is_empty() {
                break;
            }

            if !line.is_ascii() {
                findings.push(Finding::new(
                    Severity::Error,
                    "framing/header-not-ascii",
                    "the header part must be ASCII",
                    line_offset,
                ));
            }
            let text = String::from_utf8_lossy(&line);
            let Some((name, value)) = text.split_once(": ") else {
                findings.push(Finding::new(
                    Severity::Error,
                    "framing/header-field-separator",
                    format!("the header field {text:?} must separate name and value with ': '"),
                    line_offset,
                ));
                continue;
            };

            if name.eq_ignore_ascii_case("Content-Length") {
                if content_length.is_some() {
                    findings.push(Finding::new(
                        Severity::Error,
                        "framing/duplicate-content-length",
                        "the header part has more than one Content-Length field",
                        line_offset,
                    ));
                }
                if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                    findings.push(Finding::new(
                        Severity::Error,
                        "framing/content-length-not-a-number",
                        format!("Content-Length {value:?} is not a decimal number"),
                        line_offset,
                    ));
                }
                match value.trim().parse::<usize>() {
                    Ok(length) => content_length = Some(length),
                    Err(_) => {
                        return Err(FramingError::Fatal(Finding::new(
                            Severity::Error,
                            "framing/content-length-unreadable",
                            format!("Content-Length {value:?} cannot be read as a length"),
                            line_offset,
                        )));
                    }
                }
            } else if name.eq_ignore_ascii_case("Content-Type") {
                if content_type_seen {
                    findings.push(Finding::new(
                        Severity::Error,
                        "framing/duplicate-content-type",
                        "the header part has more than one Content-Type field",
                        line_offset,
                    ));
                }
                content_type_seen = true;
                check_content_type(value, line_offset, &mut findings);
            } else {
                findings.push(Finding::new(
                    Severity::Warning,
                    "framing/unknown-header-field",
                    format!("the specification defines no header field named {name:?}"),
                    line_offset,
                ));
            }
        }

        let Some(content_length) = content_length else {
            return Err(FramingError::Fatal(Finding::new(
                Severity::Error,
                "framing/missing-content-length",
                "the header part has no Content-Length field",
                start,
            )));
        };

        let body_offset = self.offset;
        let mut body = vec![0; content_length];
        if let Err(err) = self.inner.read_exact(&mut body) {
            if err.kind() == io::ErrorKind::UnexpectedEof {
                return Err(FramingError::Fatal(Finding::new(
                    Severity::Error,
                    "framing/unexpected-eof-in-body",
                    format!("the stream ended before {content_length} bytes of content"),
                    body_offset,
                )));
            }
            return Err(err.into());
        }
        self.offset += content_length as u64;

        match std::str::from_utf8(&body) {
            Err(_) => findings.push(Finding::new(
                Severity::Error,
                "framing/content-not-utf8",
                "the content is not valid UTF-8",
                body_offset,
            )),
            Ok(text) => {
                if let Err(err) = serde_json::from_str::<serde_json::Value>(text) {
                    findings.push(Finding::new(
                        Severity::Error,
                        "framing/content-not-json",
                        format!(
                            "the content is not one JSON value of exactly Content-Length bytes: {err}"
                        ),
                        body_offset,
                    ));
                }
            }
        }

        Ok(Some(Frame {
            offset: start,
            content_length,
            body,
            findings,
        }))
    }
}

fn check_content_type(value: &str, offset: u64, findings: &mut Vec<Finding>) {
    let mut parts = value.split(';').map(str::trim);
    let mime = parts.next().unwrap_or_default();
    if !mime.eq_ignore_ascii_case(DEFAULT_MIME) {
        findings.push(Finding::new(
            Severity::Warning,
            "framing/content-type-mime",
            format!("Content-Type {mime:?} is not {DEFAULT_MIME:?}"),
            offset,
        ));
    }
    for param in parts {
        let Some((key, charset)) = param.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("charset") {
            continue;
        }
        let charset = charset.trim().trim_matches('"');
        if charset.eq_ignore_ascii_case("utf-8") {
            continue;
        }
        if charset.eq_ignore_ascii_case("utf8") {
            findings.push(Finding::new(
                Severity::Warning,
                "framing/charset-utf8-alias",
                "charset \"utf8\" is accepted for backwards compatibility; the specification uses \"utf-8\"",
                offset,
            ));
        } else {
            findings.push(Finding::new(
                Severity::Error,
                "framing/charset-unsupported",
                format!("charset {charset:?} is not supported; only \"utf-8\" is"),
                offset,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(input: &[u8]) -> (Vec<Frame>, Option<FramingError>) {
        let mut reader = FrameReader::new(input);
        let mut frames = Vec::new();
        loop {
            match reader.read_frame() {
                Ok(Some(frame)) => frames.push(frame),
                Ok(None) => return (frames, None),
                Err(err) => return (frames, Some(err)),
            }
        }
    }

    fn codes(frame: &Frame) -> Vec<&'static str> {
        frame.findings.iter().map(|f| f.code).collect()
    }

    fn fatal_code(err: Option<FramingError>) -> &'static str {
        match err {
            Some(FramingError::Fatal(finding)) => finding.code,
            other => panic!("expected a fatal finding, got {other:?}"),
        }
    }

    #[test]
    fn reads_consecutive_frames_without_findings() {
        let input = b"Content-Length: 2\r\n\r\n{}Content-Length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n[]";
        let (frames, err) = read_all(input);
        assert!(err.is_none());
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].offset, 0);
        assert_eq!(frames[1].offset, 23);
        assert_eq!(frames[1].body, b"[]");
        assert!(frames.iter().all(|f| f.findings.is_empty()));
    }

    #[test]
    fn content_length_counts_utf8_bytes_not_characters() {
        let body = "{\"a\":\"日本語😀\"}";
        let ok = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let (frames, err) = read_all(ok.as_bytes());
        assert!(err.is_none());
        assert!(frames[0].findings.is_empty());

        let chars = format!("Content-Length: {}\r\n\r\n{body}", body.chars().count());
        let (frames, _) = read_all(chars.as_bytes());
        assert_eq!(codes(&frames[0]), vec!["framing/content-not-json"]);
    }

    #[test]
    fn bare_newline_is_an_error_but_still_read() {
        let (frames, err) = read_all(b"Content-Length: 2\n\n{}");
        assert!(err.is_none());
        assert_eq!(
            codes(&frames[0]),
            vec![
                "framing/header-line-terminator",
                "framing/header-line-terminator"
            ]
        );
    }

    #[test]
    fn missing_content_length_is_fatal() {
        let (_, err) = read_all(b"Content-Type: application/vscode-jsonrpc\r\n\r\n{}");
        assert_eq!(fatal_code(err), "framing/missing-content-length");
    }

    #[test]
    fn truncated_body_is_fatal() {
        let (_, err) = read_all(b"Content-Length: 10\r\n\r\n{}");
        assert_eq!(fatal_code(err), "framing/unexpected-eof-in-body");
    }

    #[test]
    fn eof_inside_header_is_fatal() {
        let (_, err) = read_all(b"Content-Length: 2\r\n");
        assert_eq!(fatal_code(err), "framing/unexpected-eof-in-header");
    }

    #[test]
    fn content_type_findings() {
        let (frames, _) = read_all(
            b"Content-Length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf8\r\n\r\n{}",
        );
        assert_eq!(codes(&frames[0]), vec!["framing/charset-utf8-alias"]);

        let (frames, _) = read_all(
            b"Content-Length: 2\r\nContent-Type: application/json; charset=latin1\r\n\r\n{}",
        );
        assert_eq!(
            codes(&frames[0]),
            vec!["framing/content-type-mime", "framing/charset-unsupported"]
        );
    }

    #[test]
    fn header_field_findings() {
        let (frames, _) = read_all(
            b"Content-Length:2\r\nContent-Length: 2\r\nContent-Length: 2\r\nX-Foo: 1\r\n\r\n{}",
        );
        assert_eq!(
            codes(&frames[0]),
            vec![
                "framing/header-field-separator",
                "framing/duplicate-content-length",
                "framing/unknown-header-field"
            ]
        );
    }

    #[test]
    fn content_length_with_sign_is_an_error() {
        let (frames, _) = read_all(b"Content-Length: +2\r\n\r\n{}");
        assert_eq!(
            codes(&frames[0]),
            vec!["framing/content-length-not-a-number"]
        );
    }
}
