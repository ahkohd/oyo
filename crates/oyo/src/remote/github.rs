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

fn gh_command() -> Command {
    let mut command = Command::new("gh");
    command
        .env_remove("GH_DEBUG")
        .env("GH_NO_UPDATE_NOTIFIER", "1");
    command
}

fn config_absent() -> bool {
    let paths = if let Some(directory) =
        std::env::var_os("GH_CONFIG_DIR").filter(|value| !value.is_empty())
    {
        vec![std::path::PathBuf::from(directory).join("hosts.yml")]
    } else {
        let (Some(home), Some(config)) = (dirs::home_dir(), dirs::config_dir()) else {
            return false;
        };
        let mut paths = vec![
            home.join(".config/gh/hosts.yml"),
            config.join("gh/hosts.yml"),
        ];
        if let Some(directory) =
            std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty())
        {
            paths.push(std::path::PathBuf::from(directory).join("gh/hosts.yml"));
        }
        if cfg!(windows) {
            if let Some(directory) = std::env::var_os("AppData").filter(|value| !value.is_empty()) {
                paths.push(std::path::PathBuf::from(directory).join("GitHub CLI/hosts.yml"));
            }
        }
        paths
    };
    paths.iter().all(|path| {
        std::fs::symlink_metadata(path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    })
}

impl ReadTransport {
    pub(crate) fn for_host(host: &str) -> Self {
        if host != "github.com"
            || ["GH_TOKEN", "GITHUB_TOKEN"]
                .iter()
                .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
        {
            return Self::AuthenticatedCli;
        }
        static AUTH: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
        let mut hosts = AUTH
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let authenticated = hosts.get(host).copied().unwrap_or_else(|| {
            let mut command = gh_command();
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
                    !config_absent()
                }
                Err(_) => {
                    let mut command = gh_command();
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
            let mut command = gh_command();
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
            let mut command = gh_command();
            command.args(["api", "--hostname", host]);
            if streaming_flag() {
                command.arg("--allow-escape-sequences");
            }
            command.args([
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

fn streaming_flag() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| {
        let mut command = gh_command();
        command.args(["api", "--help"]);
        run_command(command, None).is_ok_and(|help| help.contains("--allow-escape-sequences"))
    })
}
