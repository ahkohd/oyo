use super::{http, loading, run_command};
use crate::ReviewProviderKind;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
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

pub(crate) fn json(host: &str, endpoint: &str) -> Result<Value> {
    let transport = ReadTransport::for_host(host);
    let command = match transport {
        ReadTransport::AuthenticatedCli => {
            let mut command = Command::new("gh");
            command.args(["api", "--hostname", host, endpoint]);
            command
        }
        ReadTransport::PublicHttps => http::request(
            &format!("https://api.github.com/{endpoint}"),
            "Accept: application/vnd.github+json",
        ),
    };
    let data = match transport {
        ReadTransport::AuthenticatedCli => run_command(command, None)?,
        ReadTransport::PublicHttps => super::run_public_http(command, ReviewProviderKind::GitHub)?,
    };
    Ok(serde_json::from_str(&data)?)
}

pub(crate) fn patch(
    host: &str,
    repo: &str,
    base_tip: &str,
    head: &str,
) -> (Command, http::ResponseFormat) {
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
        ReadTransport::PublicHttps => http::request(
            &format!("https://github.com/{repo}/compare/{base_tip}...{head}.diff"),
            "Accept: text/plain",
        ),
    };
    let format = match transport {
        ReadTransport::AuthenticatedCli => http::ResponseFormat::Body,
        ReadTransport::PublicHttps => http::ResponseFormat::Headers(ReviewProviderKind::GitHub),
    };
    (command, format)
}
