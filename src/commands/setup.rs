use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use base64::Engine;

use crate::cert::{self, CertInfo};
use crate::config::{self, CONFIG_FILE, WORKFLOW_FILE};
use crate::detect;
use crate::github;
use crate::macos;
use crate::policy::{self, Channel, Kind};
use crate::workflow;

pub struct Options {
    pub channel: Channel,
    pub cert: PathBuf,
    pub kind: Option<Kind>,
    pub password_stdin: bool,
    pub apple_id: Option<String>,
    pub api_key: Option<PathBuf>,
    pub api_issuer: Option<String>,
}

pub fn run(opts: Options) -> Result<()> {
    github::require_auth()?;
    let repo = github::current_repo()?;
    let cwd = env::current_dir()?;
    let root = config::find_root(&cwd)?;

    let cert_path = cert::resolve_p12(&opts.cert, policy::application_role(opts.channel))?;
    println!("using     {}", cert_path.display());

    let password = if opts.password_stdin {
        read_stdin_password()?
    } else {
        prompt_p12_password(&cert_path)?
    };
    let infos = cert::inspect_p12(&cert_path, &password)?;
    let info = matching_cert(opts.channel, &infos)?;
    print_info(&cert_path, info);

    github::ensure_environment(&repo.name_with_owner)?;
    let (cert_secret, pass_secret) =
        policy::secrets_for_role(info.role).expect("release application certs have secret names");
    let p12_bytes = fs::read(&cert_path)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(p12_bytes);
    github::set_secret(&repo.name_with_owner, cert_secret, &b64)?;
    github::set_secret(&repo.name_with_owner, pass_secret, &password)?;
    github::set_secret(&repo.name_with_owner, "APPLE_TEAM_ID", &info.team_id)?;
    github::set_secret(
        &repo.name_with_owner,
        "APPLE_SIGNING_IDENTITY",
        &info.identity,
    )?;
    println!("secret    {cert_secret}  ({})", info.role.as_str());

    configure_notary(
        &repo.name_with_owner,
        opts.channel,
        opts.apple_id,
        opts.api_key,
        opts.api_issuer,
    )?;

    write_config(&root, opts.channel, info, opts.kind)?;
    write_channel_entitlements(&root, opts.channel, &info.team_id)?;
    write_workflow(&root)?;

    println!(
        "ok        {WORKFLOW_FILE} configured for {} ({})",
        opts.channel, repo.name_with_owner
    );
    println!("next      commit and push {WORKFLOW_FILE}, then dispatch the macOS Ship workflow on GitHub");
    Ok(())
}

fn configure_notary(
    repo: &str,
    channel: Channel,
    apple_id: Option<String>,
    api_key: Option<PathBuf>,
    api_issuer: Option<String>,
) -> Result<()> {
    if channel != Channel::DeveloperId {
        return Ok(());
    }
    let secrets = github::list_secrets(repo)?;
    let has_apple_id = secrets.iter().any(|s| s == "APPLE_ID")
        && secrets.iter().any(|s| s == "APPLE_APP_SPECIFIC_PASSWORD");
    let has_api = ["APPLE_API_KEY", "APPLE_API_ISSUER", "APPLE_API_KEY_P8"]
        .iter()
        .all(|name| secrets.iter().any(|s| s == *name));
    if has_apple_id || has_api {
        println!("notary    already configured");
        return Ok(());
    }
    if let Some(api_key) = api_key {
        let issuer =
            api_issuer.ok_or_else(|| anyhow::anyhow!("--api-key requires --api-issuer"))?;
        let key_id = key_id_from_filename(&api_key)?;
        let pem = fs::read_to_string(&api_key)?;
        github::set_secret(repo, "APPLE_API_KEY", &key_id)?;
        github::set_secret(repo, "APPLE_API_ISSUER", &issuer)?;
        github::set_secret(repo, "APPLE_API_KEY_P8", pem.trim())?;
        println!("notary    APPLE_API_KEY ({key_id})");
        return Ok(());
    }
    let email = match apple_id {
        Some(e) => e,
        None => prompt_line("Apple ID email: ")?,
    };
    if email.is_empty() {
        anyhow::bail!("Apple ID is required to notarize Developer ID builds");
    }
    if !io::stdin().is_terminal() {
        anyhow::bail!("need an app-specific password but stdin is not a TTY");
    }
    let password = rpassword::prompt_password("App-specific password (input hidden): ")?;
    if password.is_empty() {
        anyhow::bail!("app-specific password was empty");
    }
    github::set_secret(repo, "APPLE_ID", &email)?;
    github::set_secret(repo, "APPLE_APP_SPECIFIC_PASSWORD", &password)?;
    println!("notary    APPLE_ID");
    Ok(())
}

fn prompt_line(prompt: &str) -> Result<String> {
    if !io::stdin().is_terminal() {
        anyhow::bail!("need {prompt}but stdin is not a TTY");
    }
    eprint!("{prompt}");
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn key_id_from_filename(path: &Path) -> Result<String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if let Some(id) = stem.strip_prefix("AuthKey_") {
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    anyhow::bail!(
        "could not read Key ID from {}. Name it AuthKey_<KEYID>.p8",
        path.display()
    );
}

fn matching_cert(channel: Channel, infos: &[CertInfo]) -> Result<&CertInfo> {
    if let Some(info) = policy::cert_matching_channel(channel, infos) {
        return Ok(info);
    }
    let have = infos
        .iter()
        .map(|i| i.role.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "--channel {channel} requires a {} certificate; this file contains: {have}",
        policy::application_role(channel).as_str()
    );
}

fn print_info(path: &Path, info: &CertInfo) {
    println!(
        "cert      {}  {}  team {}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        info.role.as_str(),
        info.team_id
    );
}

fn prompt_p12_password(path: &Path) -> Result<String> {
    if !io::stdin().is_terminal() {
        bail!(
            "need a PKCS#12 password for {} but stdin is not a TTY. Re-run in a real terminal, or pass --password-stdin.",
            path.display()
        );
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let password = rpassword::prompt_password(format!(
        "Enter PKCS#12 password for {name} (input hidden): "
    ))?;
    if password.is_empty() {
        bail!("PKCS#12 password for {name} was empty");
    }
    Ok(password)
}

fn read_stdin_password() -> Result<String> {
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf)?;
    let password = buf.trim_end_matches(['\n', '\r']).to_string();
    if password.is_empty() {
        bail!("stdin password was empty");
    }
    Ok(password)
}

fn write_config(root: &Path, channel: Channel, info: &CertInfo, kind: Option<Kind>) -> Result<()> {
    let path = root.join(CONFIG_FILE);
    let had_file = path.exists();
    let existing = if had_file {
        Some(config::load(root)?.config)
    } else {
        None
    };
    let cfg = detect::prepare_for_setup(root, existing, kind, channel, &info.team_id)?;
    config::save(root, &cfg)?;
    println!(
        "config    {} {CONFIG_FILE}  kind={}  channels={}",
        if had_file { "updated" } else { "wrote" },
        cfg.kind,
        cfg.channels
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
            .join(",")
    );
    Ok(())
}

fn write_channel_entitlements(root: &Path, channel: Channel, team_id: &str) -> Result<()> {
    let dir = root.join("packaging");
    fs::create_dir_all(&dir)?;
    match channel {
        Channel::DeveloperId => {
            let path = dir.join("developer-id.entitlements");
            if !path.exists() {
                fs::write(&path, macos::default_entitlements())?;
                println!("wrote     {}", path.display());
            }
        }
        Channel::AppStore => {
            let bundle_id = config::load(root)?.config.bundle_id;
            let path = dir.join("app-store.entitlements");
            if !path.exists() {
                fs::write(
                    &path,
                    macos::app_store_entitlements_template(team_id, &bundle_id),
                )?;
                println!("wrote     {}", path.display());
            }
        }
    }
    Ok(())
}

fn write_workflow(root: &Path) -> Result<()> {
    let located = config::load(root)?;
    if located.config.channels.is_empty() {
        bail!("{CONFIG_FILE} has no channels; setup did not record the channel");
    }
    let path = root.join(WORKFLOW_FILE);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(
        &path,
        workflow::render(located.config.kind, &located.config.channels),
    )?;
    println!("workflow  wrote {WORKFLOW_FILE}");
    Ok(())
}
