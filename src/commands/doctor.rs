use std::env;

use anyhow::{bail, Result};

use crate::config::{self, WORKFLOW_FILE};
use crate::github;
use crate::policy::{self, Channel};

pub fn run(channel: Option<Channel>) -> Result<()> {
    let cwd = env::current_dir()?;
    let located = config::load(&cwd)?;
    println!("config    {}", located.path.display());
    println!("kind      {}", located.config.kind);
    println!("team      {}", located.config.team_id);
    println!("bundle    {}", located.config.bundle_id);
    println!("product   {}", located.config.product_name);

    match located.config.kind {
        crate::policy::Kind::Gpui => {
            let _ = located.config.gpui()?;
        }
        crate::policy::Kind::Tauri => {
            let _ = located.config.tauri()?;
        }
        crate::policy::Kind::Native => {
            let _ = located.config.native()?;
        }
    }

    let workflow = located.root.join(WORKFLOW_FILE);
    if workflow.is_file() {
        println!("workflow  {}", workflow.display());
    } else {
        bail!("missing {WORKFLOW_FILE}. Run `apple-ship setup --cert <p12>`.");
    }

    github::require_auth()?;
    let repo = github::current_repo()?;
    println!("github    {}", repo.name_with_owner);

    let remote =
        github::remote_workflow_exists(&repo.name_with_owner, &repo.default_branch, WORKFLOW_FILE)?;
    if remote {
        println!("remote    {WORKFLOW_FILE} on {}", repo.default_branch);
    } else {
        println!(
            "remote    {WORKFLOW_FILE} is NOT on {} — push it before `apple-ship ship`",
            repo.default_branch
        );
    }

    let secrets = github::list_secrets(&repo.name_with_owner)?;
    let channels = match channel {
        Some(ch) => vec![ch],
        None => vec![Channel::DeveloperId, Channel::AppStore],
    };
    let mut failed = false;
    for ch in channels {
        if !check_channel(ch, &secrets, channel.is_none()) {
            failed = true;
        }
        if ch == Channel::DeveloperId && !has_notary(&secrets) {
            println!("notary    missing (need APPLE_ID + APPLE_APP_SPECIFIC_PASSWORD, or API key)");
            if channel == Some(Channel::DeveloperId) {
                failed = true;
            }
        } else if ch == Channel::DeveloperId {
            println!("notary    ok");
        }
        if ch == Channel::AppStore && !policy::ci_supports(ch) {
            println!(
                "channel   app-store secrets can be stored; ci shipping is not implemented yet"
            );
        }
    }

    if failed {
        bail!("doctor found missing secrets or files");
    }
    println!("ok");
    Ok(())
}

fn check_channel(channel: Channel, secrets: &[String], optional_app_store: bool) -> bool {
    let required = policy::required_signing_secrets(channel);
    let missing: Vec<_> = required
        .iter()
        .copied()
        .filter(|name| !secrets.iter().any(|s| s == name))
        .collect();
    if missing.is_empty() {
        println!("channel   {channel} secrets ok");
        true
    } else if optional_app_store && channel == Channel::AppStore {
        println!(
            "channel   app-store not fully configured ({})",
            missing.join(", ")
        );
        true
    } else {
        println!("channel   {channel} missing {}", missing.join(", "));
        false
    }
}

fn has_notary(secrets: &[String]) -> bool {
    policy::notary_secret_groups()
        .iter()
        .any(|group| group.iter().all(|name| secrets.iter().any(|s| s == *name)))
}
