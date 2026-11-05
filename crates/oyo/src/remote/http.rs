use crate::ReviewProviderKind;
use std::collections::HashMap;
use std::io::{self, BufRead, Read};
use std::process::Command;

#[derive(Clone, Copy)]
pub(crate) enum ResponseFormat {
    Body,
    Headers(ReviewProviderKind),
}

fn provider_label(provider: ReviewProviderKind) -> &'static str {
    match provider {
        ReviewProviderKind::GitHub => "GitHub",
        ReviewProviderKind::GitLab => "GitLab",
        ReviewProviderKind::Forgejo => "Forgejo",
    }
}

pub(crate) struct ReadRequest {
    pub(super) command: Command,
    pub(super) input: Option<String>,
    pub(super) format: ResponseFormat,
}

pub(crate) struct Response {
    pub(crate) body: Vec<u8>,
    pub(crate) headers: HashMap<String, String>,
}

impl ReadRequest {
    pub(crate) fn with_curl_config(mut self, input: String) -> Self {
        self.command.args(["--config", "-"]);
        self.input = Some(input);
        self
    }

    pub(crate) fn execute(self) -> anyhow::Result<Response> {
        super::run_response(self.command, self.input.as_deref(), self.format)
    }

    pub(crate) fn public(url: &str, accept: &str, provider: ReviewProviderKind) -> Self {
        Self {
            command: request(url, accept),
            input: None,
            format: ResponseFormat::Headers(provider),
        }
    }
}

pub(crate) struct ResponseBody<R> {
    reader: R,
    format: ResponseFormat,
    headers: HashMap<String, String>,
}

impl<R: BufRead> ResponseBody<R> {
    pub(crate) fn new(reader: R, format: ResponseFormat) -> Self {
        Self {
            reader,
            format,
            headers: HashMap::new(),
        }
    }

    pub(super) fn take_headers(&mut self) -> HashMap<String, String> {
        std::mem::take(&mut self.headers)
    }

    fn read_headers(&mut self) -> io::Result<()> {
        let ResponseFormat::Headers(provider) = self.format else {
            return Ok(());
        };
        let label = provider_label(provider);
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
                            format!("{label} response headers stopped early"),
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
                            format!("{label} response headers exceed the 64 KiB loading limit"),
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
                        format!("Invalid {label} HTTP status"),
                    )
                })?;
            if (100..200).contains(&status) {
                continue;
            }
            if let Some(error) = response_failure(text, provider) {
                return Err(io::Error::other(error));
            }
            for line in text.lines().skip(1) {
                if let Some((name, value)) = line.split_once(':') {
                    let name = name.to_ascii_lowercase();
                    if name == "link" {
                        self.headers
                            .entry(name)
                            .and_modify(|previous| {
                                previous.push(',');
                                previous.push_str(value.trim());
                            })
                            .or_insert_with(|| value.trim().to_owned());
                    } else if self
                        .headers
                        .insert(name.clone(), value.trim().to_owned())
                        .is_some()
                        && matches!(name.as_str(), "x-next-page" | "x-total" | "x-page")
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Repeated HTTP pagination header",
                        ));
                    }
                }
            }
            self.format = ResponseFormat::Body;
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

fn response_failure(headers: &str, provider: ReviewProviderKind) -> Option<String> {
    let label = provider_label(provider);
    let auth = match provider {
        ReviewProviderKind::GitHub => "an existing gh login",
        ReviewProviderKind::GitLab => "configured GitLab authentication",
        ReviewProviderKind::Forgejo => "configured Forgejo authentication",
    };
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
            && (fields
                .get("x-ratelimit-remaining")
                .or_else(|| fields.get("ratelimit-remaining"))
                == Some(&"0")
                || fields.contains_key("retry-after"))
    {
        let retry = fields
            .get("retry-after")
            .map(|v| format!(" Retry-After: {}.", oyo_core::patch::terminal_text(v)))
            .unwrap_or_default();
        let reset = fields
            .get("x-ratelimit-reset")
            .or_else(|| fields.get("ratelimit-reset"))
            .map(|v| {
                format!(
                    " Rate limit resets at Unix time {}.",
                    oyo_core::patch::terminal_text(v)
                )
            })
            .unwrap_or_default();
        return Some(format!("{label} rate limit reached (HTTP {status}).{retry}{reset} Wait before retrying. Use {auth} for authenticated access."));
    }
    Some(match status {
        404 => format!("{label} resource is unavailable or private (HTTP 404). Private repositories require {auth}."),
        _ => format!("{label} read failed (HTTP {status})."),
    })
}

pub(crate) fn request(url: &str, accept: &str) -> Command {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn metadata_http_error_stops_a_slow_body_and_keeps_retry_details() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "printf 'HTTP/2 429\r\nRetry-After: 90\r\n\r\n'; sleep 60",
        ]);
        let start = std::time::Instant::now();
        let error = super::super::run_public_http(command, ReviewProviderKind::GitHub)
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
            let error=response_failure(&format!("HTTP/1.1 200 Connection established\r\n\r\nHTTP/2 {status}\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 1793833862\r\nRetry-After: 90\r\n\r\n"), ReviewProviderKind::GitHub).unwrap();
            assert!(error.contains("rate limit reached"));
            assert!(error.contains("1793833862"));
            assert!(error.contains("Retry-After: 90"));
        }
        assert!(response_failure(
            "HTTP/2 200\r\nx-ratelimit-remaining: 0\r\n\r\n",
            ReviewProviderKind::GitHub
        )
        .is_none());
        assert!(
            response_failure("HTTP/2 404\r\n\r\n", ReviewProviderKind::GitHub)
                .unwrap()
                .contains("unavailable or private")
        );
        assert!(
            !response_failure("HTTP/2 403\r\n\r\n", ReviewProviderKind::GitHub)
                .unwrap()
                .contains("rate limit reached")
        );
        assert!(response_failure(
            "HTTP/2 302\r\nRetry-After: 90\r\n\r\nHTTP/2 200\r\n\r\n",
            ReviewProviderKind::GitHub
        )
        .is_none());
    }

    #[test]
    fn public_response_headers_are_bounded_before_allocation_and_body_parsing() {
        for capacity in 1..32 {
            let data=b"HTTP/1.1 103 Early Hints\r\nLink: fixture\r\n\r\nHTTP/2 200\r\nX-RateLimit-Remaining: 0\r\n\r\n{\"ok\":true}";
            let reader = std::io::BufReader::with_capacity(capacity, data.as_slice());
            let value: Value = serde_json::from_reader(ResponseBody::new(
                reader,
                ResponseFormat::Headers(ReviewProviderKind::GitHub),
            ))
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
            let mut body = ResponseBody::new(
                std::io::Cursor::new(data),
                ResponseFormat::Headers(ReviewProviderKind::GitHub),
            );
            assert!(body.read_to_end(&mut Vec::new()).is_err());
        }
        let command = request(
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
