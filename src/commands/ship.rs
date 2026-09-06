use anyhow::Result;

use crate::config::WORKFLOW_FILE;
use crate::github;
use crate::policy::Channel;

pub fn run(channel: Channel) -> Result<()> {
    github::require_auth()?;
    let repo = github::current_repo()?;
    println!("dispatch  {}  channel={channel}", repo.name_with_owner);
    let workflow = std::path::Path::new(WORKFLOW_FILE)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("macos-ship.yml");
    github::dispatch_and_watch(workflow, channel.as_str())?;
    Ok(())
}
