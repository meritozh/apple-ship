use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::config::RELEASE_ENV;

#[derive(Clone, Debug)]
pub struct GhRepo {
    pub name_with_owner: String,
    pub default_branch: String,
}

pub fn require_auth() -> Result<()> {
    let output = gh(&["auth", "status"])?;
    if !output.status.success() {
        bail!("gh is not authenticated. Run `gh auth login` first.");
    }
    Ok(())
}

pub fn current_repo() -> Result<GhRepo> {
    let output = gh(&["repo", "view", "--json", "nameWithOwner,defaultBranchRef"])?;
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
    let default_branch = v
        .pointer("/defaultBranchRef/name")
        .and_then(|x| x.as_str())
        .unwrap_or("main")
        .to_string();
    Ok(GhRepo {
        name_with_owner,
        default_branch,
    })
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
        // Older gh or empty env: fall back to TSV list.
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

pub fn remote_workflow_exists(repo: &str, branch: &str, path: &str) -> Result<bool> {
    let api = format!("repos/{repo}/contents/{path}?ref={branch}");
    let output = gh(&["api", "-X", "GET", &api])?;
    Ok(output.status.success())
}

pub fn dispatch_and_watch(workflow: &str, channel: &str) -> Result<()> {
    let before = latest_run_id(workflow)?;
    let output = gh(&[
        "workflow",
        "run",
        workflow,
        "-f",
        &format!("channel={channel}"),
    ])?;
    if !output.status.success() {
        bail!(
            "gh workflow run {workflow} failed: {}. Is the workflow on the default branch?",
            stderr(&output)
        );
    }
    let mut run_id = None;
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(500));
        let id = latest_run_id(workflow)?;
        if id != before {
            run_id = id;
            break;
        }
    }
    let Some(id) = run_id else {
        bail!("timed out waiting for the Actions run to appear. Check `gh run list --workflow {workflow}`.");
    };
    println!("watching run {id}");
    let status = Command::new("gh")
        .args(["run", "watch", &id, "--exit-status"])
        .status()
        .context("gh run watch")?;
    if !status.success() {
        let _ = Command::new("gh")
            .args(["run", "view", &id, "--log-failed"])
            .status();
        bail!("workflow run {id} failed. Open it with: gh run view {id} --web");
    }
    println!("run {id} completed");
    Ok(())
}

fn latest_run_id(workflow: &str) -> Result<Option<String>> {
    let output = gh(&[
        "run",
        "list",
        "--workflow",
        workflow,
        "--json",
        "databaseId",
        "--limit",
        "1",
    ])?;
    if !output.status.success() {
        return Ok(None);
    }
    let v: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Array(vec![]));
    Ok(v.as_array()
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("databaseId"))
        .and_then(|id| {
            id.as_u64()
                .map(|n| n.to_string())
                .or_else(|| id.as_str().map(|s| s.to_string()))
        }))
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
