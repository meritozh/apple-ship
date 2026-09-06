use std::env;

use anyhow::{bail, Result};

use crate::bundle;
use crate::config;
use crate::macos;
use crate::policy::{self, Channel};

pub fn run(channel: Channel) -> Result<()> {
    macos::require_github_actions()?;
    if !policy::ci_supports(channel) {
        bail!(
            "this apple-ship build ships {channel} artifacts in a later phase. Developer ID is implemented; App Store pkg export is not."
        );
    }
    let cwd = env::current_dir()?;
    let located = config::load(&cwd)?;
    let config = located.config;

    let role = policy::application_role(channel);
    let (cert_var, pass_var) = policy::secrets_for_role(role)
        .ok_or_else(|| anyhow::anyhow!("no signing secrets for {channel}"))?;
    let p12 = macos::env_nonempty(cert_var)
        .ok_or_else(|| anyhow::anyhow!("{cert_var} is missing or empty"))?;
    let password = macos::env_nonempty(pass_var)
        .ok_or_else(|| anyhow::anyhow!("{pass_var} is missing or empty"))?;
    macos::import_certificate_p12(&p12, &password)?;

    println!(
        "shipping  {} ({})  channel={channel}  team={}",
        config.product_name, config.kind, config.team_id
    );
    let artifact = bundle::ship_developer_id(&located.root, &config)?;
    macos::write_github_output("artifact", &artifact.to_string_lossy())?;
    macos::write_github_output("channel", channel.as_str())?;
    println!("artifact  {}", artifact.display());
    Ok(())
}
