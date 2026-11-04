use super::{loading, run_command};
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, BufRead, Read};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadTransport {
    AuthenticatedCli,
    PublicHttps,
}

impl ReadTransport {
    pub(crate) fn for_host(host: &str) -> Self {
        if host != "github.com" {
            return Self::AuthenticatedCli;
        }
        static AUTH: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
        let mut hosts = AUTH
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let authenticated = hosts.get(host).copied().unwrap_or_else(|| {
            let mut command = Command::new("gh");
            command.args([
                "auth",
                "status",
                "--hostname",
                host,
                "--json",
                "hosts",
                "--jq",
                ".hosts | length > 0",
            ]);
            let authenticated = match run_command(command, None) {
                Ok(output) => output.trim().parse::<bool>().unwrap_or(true),
                Err(error)
                    if error.chain().any(|e| {
                        e.downcast_ref::<std::io::Error>()
                            .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                    }) =>
                {
                    false
                }
                Err(_) => {
                    let mut command = Command::new("gh");
                    command.args(["auth", "status", "--hostname", host]);
                    match run_command(command, None) {
                        Ok(_) => true,
                        Err(error) => !error
                            .to_string()
                            .contains("You are not logged into any GitHub hosts."),
                    }
                }
            };
            if !loading::cancelled() {
                hosts.insert(host.to_owned(), authenticated);
            }
            authenticated
        });
        if authenticated {
            Self::AuthenticatedCli
        } else {
            Self::PublicHttps
        }
    }
}

pub(crate) struct ResponseBody<R> {
    reader: R,
    headers_done: bool,
}

impl<R: BufRead> ResponseBody<R> {
    pub(crate) fn new(reader: R, transport: ReadTransport) -> Self {
        Self {
            reader,
            headers_done: transport == ReadTransport::AuthenticatedCli,
        }
    }

    fn read_headers(&mut self) -> io::Result<()> {
        if self.headers_done {
            return Ok(());
        }
        let mut total = 0usize;
        loop {
            let mut block = Vec::new();
            loop {
                let start = block.len();
                loop {
                    let bytes = self.reader.fill_buf()?;
                    if bytes.is_empty() {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "Public GitHub response headers stopped early",
                        ));
                    }
                    let end = bytes
                        .iter()
                        .position(|b| *b == b'\n')
                        .map_or(bytes.len(), |n| n + 1);
                    total += end;
                    if total > 65536 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Public GitHub response headers exceed the 64 KiB loading limit",
                        ));
                    }
                    let newline = bytes[end - 1] == b'\n';
                    block.extend_from_slice(&bytes[..end]);
                    self.reader.consume(end);
                    if newline {
                        break;
                    }
                }
                if matches!(&block[start..], b"\r\n" | b"\n") {
                    break;
                }
            }
            let text = std::str::from_utf8(&block)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            let status = text
                .lines()
                .next()
                .filter(|line| line.starts_with("HTTP/"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u16>().ok())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Invalid public GitHub HTTP status",
                    )
                })?;
            if (100..200).contains(&status) {
                continue;
            }
            if let Some(error) = response_failure(text) {
                return Err(io::Error::other(error));
            }
            self.headers_done = true;
            return Ok(());
        }
    }
}

impl<R: BufRead> Read for ResponseBody<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.read_headers()?;
        self.reader.read(output)
    }
}

impl<R: BufRead> BufRead for ResponseBody<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.read_headers()?;
        self.reader.fill_buf()
    }
    fn consume(&mut self, count: usize) {
        self.reader.consume(count)
    }
}

fn response_failure(headers: &str) -> Option<String> {
    let mut status = 0u16;
    let mut fields = HashMap::new();
    for line in headers.lines() {
        if line.starts_with("HTTP/") {
            status = line
                .split_whitespace()
                .nth(1)
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            fields.clear();
        } else if let Some((name, value)) = line.split_once(':') {
            fields.insert(name.to_ascii_lowercase(), value.trim());
        }
    }
    if (200..300).contains(&status) || status == 0 {
        return None;
    }
    if status == 429
        || status == 403
            && (fields.get("x-ratelimit-remaining") == Some(&"0")
                || fields.contains_key("retry-after"))
    {
        let retry = fields
            .get("retry-after")
            .map(|v| format!(" Retry-After: {}.", oyo_core::patch::terminal_text(v)))
            .unwrap_or_default();
        let reset = fields
            .get("x-ratelimit-reset")
            .map(|v| {
                format!(
                    " Rate limit resets at Unix time {}.",
                    oyo_core::patch::terminal_text(v)
                )
            })
            .unwrap_or_default();
        return Some(format!("Public GitHub rate limit reached (HTTP {status}).{retry}{reset} Wait before retrying. An existing gh login can provide higher limits."));
    }
    Some(match status {
        404 => "Public GitHub resource is unavailable or private (HTTP 404). Private repositories require an existing gh login.".to_owned(),
        _ => format!("Public GitHub read failed (HTTP {status})."),
    })
}

fn public_request(url: &str, accept: &str) -> Command {
    let mut command = Command::new("curl");
    command.args([
        "--disable",
        "--no-buffer",
        "--silent",
        "--show-error",
        "--dump-header",
        "-",
        "--suppress-connect-headers",
        "--proto",
        "=https",
        "--connect-timeout",
        "10",
        "--max-time",
        "30",
        "--user-agent",
        "Oyo",
        "--header",
        accept,
        url,
    ]);
    command
}

pub(crate) fn json(host: &str, endpoint: &str) -> Result<Value> {
    let transport = ReadTransport::for_host(host);
    let command = match transport {
        ReadTransport::AuthenticatedCli => {
            let mut command = Command::new("gh");
            command.args(["api", "--hostname", host, endpoint]);
            command
        }
        ReadTransport::PublicHttps => public_request(
            &format!("https://api.github.com/{endpoint}"),
            "Accept: application/vnd.github+json",
        ),
    };
    let data = match transport {
        ReadTransport::AuthenticatedCli => run_command(command, None)?,
        ReadTransport::PublicHttps => super::run_public_http(command)?,
    };
    Ok(serde_json::from_str(&data)?)
}

pub(crate) fn patch(
    host: &str,
    repo: &str,
    base_tip: &str,
    head: &str,
) -> (Command, ReadTransport) {
    let transport = ReadTransport::for_host(host);
    let command = match transport {
        ReadTransport::AuthenticatedCli => {
            let mut command = Command::new("gh");
            command.args([
                "api",
                "--hostname",
                host,
                "--allow-escape-sequences",
                "-H",
                "Accept: application/vnd.github.diff",
                &format!("repos/{repo}/compare/{base_tip}...{head}"),
            ]);
            command
        }
        ReadTransport::PublicHttps => public_request(
            &format!("https://github.com/{repo}/compare/{base_tip}...{head}.diff"),
            "Accept: text/plain",
        ),
    };
    (command, transport)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_http_error_stops_a_slow_body_and_keeps_retry_details() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "printf 'HTTP/2 429\r\nRetry-After: 90\r\n\r\n'; sleep 60",
        ]);
        let start = std::time::Instant::now();
        let error = super::super::run_public_http(command)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("rate limit reached") && error.contains("Retry-After: 90"),
            "{error}"
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn public_http_errors_preserve_rate_limits_without_automatic_retries() {
        for status in [403, 429] {
            let error=response_failure(&format!("HTTP/1.1 200 Connection established\r\n\r\nHTTP/2 {status}\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 1793833862\r\nRetry-After: 90\r\n\r\n")).unwrap();
            assert!(error.contains("rate limit reached"));
            assert!(error.contains("1793833862"));
            assert!(error.contains("Retry-After: 90"));
        }
        assert!(response_failure("HTTP/2 200\r\nx-ratelimit-remaining: 0\r\n\r\n").is_none());
        assert!(response_failure("HTTP/2 404\r\n\r\n")
            .unwrap()
            .contains("unavailable or private"));
        assert!(!response_failure("HTTP/2 403\r\n\r\n")
            .unwrap()
            .contains("rate limit reached"));
        assert!(
            response_failure("HTTP/2 302\r\nRetry-After: 90\r\n\r\nHTTP/2 200\r\n\r\n").is_none()
        );
    }

    #[test]
    fn public_response_headers_are_bounded_before_allocation_and_body_parsing() {
        for capacity in 1..32 {
            let data=b"HTTP/1.1 103 Early Hints\r\nLink: fixture\r\n\r\nHTTP/2 200\r\nX-RateLimit-Remaining: 0\r\n\r\n{\"ok\":true}";
            let reader = std::io::BufReader::with_capacity(capacity, data.as_slice());
            let value: Value =
                serde_json::from_reader(ResponseBody::new(reader, ReadTransport::PublicHttps))
                    .unwrap();
            assert_eq!(value["ok"], true);
        }
        for data in [
            b"HTTP/2 200\r\nX: "
                .iter()
                .copied()
                .chain(std::iter::repeat_n(b'a', 65537))
                .collect::<Vec<_>>(),
            b"HTTP/2 302\r\nLocation: https://other.example\r\n\r\n".to_vec(),
            b"HTTP/2 200\r\nX: truncated".to_vec(),
        ] {
            let mut body =
                ResponseBody::new(std::io::Cursor::new(data), ReadTransport::PublicHttps);
            assert!(body.read_to_end(&mut Vec::new()).is_err());
        }
        let command = public_request(
            "https://api.github.com/repos/o/r/pulls/1",
            "Accept: application/vnd.github+json",
        );
        let args = command
            .get_args()
            .map(|v| v.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args[0], "--disable");
        assert!(args.iter().any(|v| v == "--no-buffer"));
        assert!(args.windows(2).any(|v| v == ["--dump-header", "-"]));
        assert!(!args
            .iter()
            .any(|v| v == "--location" || v.contains("Authorization") || v.contains("netrc")));
    }
}
