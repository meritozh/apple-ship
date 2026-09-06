use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
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
}

pub fn run(opts: Options) -> Result<()> {
    github::require_auth()?;
    let repo = github::current_repo()?;
    let cwd = env::current_dir()?;
    let root = config::find_root(&cwd)?;

    let cert_path = opts
        .cert
        .canonicalize()
        .with_context(|| format!("certificate not found: {}", opts.cert.display()))?;
    let ext = cert_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "p12" && ext != "pfx" {
        bail!(
            "--cert must be a PKCS#12 file (.p12), got {}. {} needs a {} certificate with its private key.",
            cert_path.display(),
            opts.channel,
            policy::application_role(opts.channel).as_str()
        );
    }

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
