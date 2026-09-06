use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::config::RELEASE_ENV;

#[derive(Clone, Debug)]
pub struct GhRepo {
    pub name_with_owner: String,
}

pub fn require_auth() -> Result<()> {
    let output = gh(&["auth", "status"])?;
    if !output.status.success() {
        bail!("gh is not authenticated. Run `gh auth login` first.");
    }
    Ok(())
}

pub fn current_repo() -> Result<GhRepo> {
    let output = gh(&["repo", "view", "--json", "nameWithOwner"])?;
    if !output.status.success() {
        bail!(
            "not a GitHub repo, or gh cannot see it: {}",
            stderr(&output)
        );
    }
    let v: Value = serde_json::from_slice(&output.stdout).context("gh repo view json")?;
    let name_with_owner = v
        .get("nameWithOwner")
        .and_then(|x| x.as_str())
        .context("missing nameWithOwner")?
        .to_string();
    Ok(GhRepo { name_with_owner })
}

pub fn ensure_environment(repo: &str) -> Result<()> {
    let path = format!("repos/{repo}/environments/{RELEASE_ENV}");
    let output = gh(&["api", "--method", "PUT", &path])?;
    if !output.status.success() {
        bail!(
            "could not create GitHub environment `{RELEASE_ENV}` on {repo}: {}",
            stderr(&output)
        );
    }
    Ok(())
}

pub fn set_secret(repo: &str, name: &str, value: &str) -> Result<()> {
    let mut child = Command::new("gh")
        .args(["secret", "set", name, "--repo", repo, "--env", RELEASE_ENV])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn `gh secret set`")?;
    child
        .stdin
        .as_mut()
        .context("gh stdin")?
        .write_all(value.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "gh secret set {name} --env {RELEASE_ENV} failed: {}",
            stderr(&output)
        );
    }
    Ok(())
}

pub fn list_secrets(repo: &str) -> Result<Vec<String>> {
    let output = gh(&[
        "secret",
        "list",
        "--repo",
        repo,
        "--env",
        RELEASE_ENV,
        "--json",
        "name",
    ])?;
    if !output.status.success() {
        let output = gh(&["secret", "list", "--repo", repo, "--env", RELEASE_ENV])?;
        if !output.status.success() {
            return Ok(Vec::new());
        }
        let text = String::from_utf8_lossy(&output.stdout);
        return Ok(text
            .lines()
            .filter_map(|line| line.split_whitespace().next().map(|s| s.to_string()))
            .filter(|s| s != "NAME")
            .collect());
    }
    let v: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Array(vec![]));
    Ok(v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("name")?.as_str().map(|s| s.to_string()))
        .collect())
}

fn gh(args: &[&str]) -> Result<std::process::Output> {
    Command::new("gh")
        .args(args)
        .output()
        .context("failed to run gh; install GitHub CLI and authenticate")
}

fn stderr(output: &std::process::Output) -> String {
    let s = String::from_utf8_lossy(&output.stderr);
    let t = s.trim();
    if t.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        t.to_string()
    }
}
