use super::{http, loading, run_command};
use crate::ReviewProviderKind;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

fn glab_config_absent() -> bool {
    let mut paths = Vec::new();
    if let Some(directory) = std::env::var_os("GLAB_CONFIG_DIR").filter(|value| !value.is_empty()) {
        paths.push(std::path::PathBuf::from(directory).join("config.yml"));
    } else {
        let (Some(home), Some(config)) = (dirs::home_dir(), dirs::config_dir()) else {
            return false;
        };
        paths.push(home.join(".config/glab-cli/config.yml"));
        paths.push(config.join("glab-cli/config.yml"));
        if let Some(directory) =
            std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty())
        {
            paths.push(std::path::PathBuf::from(directory).join("glab-cli/config.yml"));
        }
        if let Some(directories) =
            std::env::var_os("XDG_CONFIG_DIRS").filter(|value| !value.is_empty())
        {
            paths.extend(
                std::env::split_paths(&directories)
                    .map(|directory| directory.join("glab-cli/config.yml")),
            );
        } else if cfg!(target_os = "macos") {
            paths.extend(
                [
                    home.join("Library/Preferences"),
                    std::path::PathBuf::from("/Library/Application Support"),
                    std::path::PathBuf::from("/Library/Preferences"),
                ]
                .into_iter()
                .map(|directory| directory.join("glab-cli/config.yml")),
            );
        } else if cfg!(unix) {
            paths.push(std::path::PathBuf::from("/etc/xdg/glab-cli/config.yml"));
        } else {
            return false;
        }
    }
    let mut command = Command::new("git");
    command.args(["rev-parse", "--git-dir"]);
    let directory = match run_command(command, None) {
        Ok(directory) => std::path::PathBuf::from(directory.trim()),
        Err(error)
            if error.to_string().contains("not a git repository")
                || error.chain().any(|cause| {
                    cause
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                }) =>
        {
            std::path::PathBuf::from(".git")
        }
        Err(_) => return false,
    };
    paths.push(directory.join("glab-cli/config.yml"));
    paths.iter().all(|path| {
        std::fs::symlink_metadata(path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    })
}

pub(crate) fn anonymous(host: &str) -> bool {
    if [
        "GITLAB_TOKEN",
        "GITLAB_ACCESS_TOKEN",
        "OAUTH_TOKEN",
        "CI_JOB_TOKEN",
        "GLAB_IS_OAUTH2",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        return false;
    }
    static HOSTS: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let mut hosts = HOSTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(anonymous) = hosts.get(host) {
        return *anonymous;
    }
    let anonymous = glab_config_absent();
    if !loading::cancelled() {
        hosts.insert(host.to_owned(), anonymous);
    }
    anonymous
}

pub(crate) fn json(host: &str, endpoint: &str) -> Result<Value> {
    if anonymous(host) {
        http::json(
            &format!("https://{host}/api/v4/{endpoint}"),
            ReviewProviderKind::GitLab,
        )
    } else {
        let mut command = Command::new("glab");
        command.args(["api", "--hostname", host, endpoint]);
        Ok(serde_json::from_str(&run_command(command, None)?)?)
    }
}
