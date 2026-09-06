use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use base64::Engine;

use crate::cert::{self, CertInfo, CertRole};
use crate::config::{self, CONFIG_FILE, WORKFLOW_FILE};
use crate::detect;
use crate::github;
use crate::macos;
use crate::policy;
use crate::workflow;

pub struct Options {
    pub certs: Vec<PathBuf>,
    pub api_key: Option<PathBuf>,
    pub api_issuer: Option<String>,
    pub apple_id: Option<String>,
    pub password_stdin: bool,
    pub force: bool,
    pub push: bool,
}

pub fn run(opts: Options) -> Result<()> {
    github::require_auth()?;
    let repo = github::current_repo()?;
    let cwd = env::current_dir()?;
    let root = config::find_root(&cwd)?;

    let stdin_password = if opts.password_stdin {
        Some(read_stdin_password()?)
    } else {
        None
    };

    let mut team_id: Option<String> = None;
    let mut application_identity: Option<String> = None;
    let mut saw_release_cert = false;

    for cert_path in &opts.certs {
        let cert_path = cert_path
            .canonicalize()
            .with_context(|| format!("certificate not found: {}", cert_path.display()))?;
        let ext = cert_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if matches!(ext.as_str(), "cer" | "crt" | "der" | "pem") {
            let info = cert::inspect_x509(&cert_path)?;
            print_info(&cert_path, &info);
            reject_development(&info)?;
            bail!(
                "{} is a public certificate. CI signing needs a .p12 that includes the private key. In Keychain Access: My Certificates → export `{}.p12`.",
                cert_path.display(),
                info.identity
            );
        }

        let password = match stdin_password.as_deref() {
            Some(p) => p.to_string(),
            None => prompt_p12_password(&cert_path)?,
        };
        let infos = cert::inspect_p12(&cert_path, &password)?;
        for info in &infos {
            print_info(&cert_path, info);
            reject_development(info)?;
        }
        if team_id.is_none() {
            team_id = infos.first().map(|i| i.team_id.clone());
        }
        for info in &infos {
            if team_id.as_deref() != Some(info.team_id.as_str()) {
                bail!(
                    "team ID mismatch: {} vs {}",
                    team_id.as_deref().unwrap_or("?"),
                    info.team_id
                );
            }
            if info.role == CertRole::DeveloperIdApplication
                || info.role == CertRole::AppleDistribution
            {
                application_identity.get_or_insert_with(|| info.identity.clone());
                saw_release_cert = true;
            }
            let Some((cert_secret, pass_secret)) = policy::secrets_for_role(info.role) else {
                continue;
            };
            let p12_bytes = fs::read(&cert_path)?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(p12_bytes);
            github::ensure_environment(&repo.name_with_owner)?;
            github::set_secret(&repo.name_with_owner, cert_secret, &b64)?;
            github::set_secret(&repo.name_with_owner, pass_secret, &password)?;
            println!("secret    {cert_secret}  ({})", info.role.as_str());
        }
    }

    if !saw_release_cert {
        bail!("no Developer ID Application or Apple Distribution certificate was provided");
    }
    let team_id = team_id.context("could not read Team ID from certificate")?;

    github::ensure_environment(&repo.name_with_owner)?;
    github::set_secret(&repo.name_with_owner, "APPLE_TEAM_ID", &team_id)?;
    if let Some(identity) = &application_identity {
        github::set_secret(&repo.name_with_owner, "APPLE_SIGNING_IDENTITY", identity)?;
    }

    if let Some(api_key) = opts.api_key {
        let issuer = opts
            .api_issuer
            .as_deref()
            .context("--api-key requires --api-issuer <uuid>")?;
        let key_id = key_id_from_path(&api_key)?;
        let pem =
            fs::read_to_string(&api_key).with_context(|| format!("read {}", api_key.display()))?;
        github::set_secret(&repo.name_with_owner, "APPLE_API_KEY", &key_id)?;
        github::set_secret(&repo.name_with_owner, "APPLE_API_ISSUER", issuer)?;
        github::set_secret(&repo.name_with_owner, "APPLE_API_KEY_P8", pem.trim())?;
        println!("secret    APPLE_API_KEY  ({key_id})");
    }
    if let Some(apple_id) = opts.apple_id {
        github::set_secret(&repo.name_with_owner, "APPLE_ID", &apple_id)?;
        println!("secret    APPLE_ID");
        println!(
            "note      set APPLE_APP_SPECIFIC_PASSWORD with: gh secret set APPLE_APP_SPECIFIC_PASSWORD --env release"
        );
    }

    write_config(&root, &team_id, opts.force)?;
    write_packaging_templates(&root, &team_id)?;
    write_workflow(&root, opts.force)?;

    if opts.push {
        push_workflow(&root)?;
    } else {
        println!(
            "next      commit and push {WORKFLOW_FILE}, then: apple-ship ship --channel developer-id"
        );
    }
    Ok(())
}

fn print_info(path: &Path, info: &CertInfo) {
    println!(
        "cert      {}  {}  team {}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        info.role.as_str(),
        info.team_id
    );
}

fn reject_development(info: &CertInfo) -> Result<()> {
    if info.role == CertRole::AppleDevelopment {
        bail!(
            "`{}` is an Apple Development certificate and cannot ship. Use Developer ID Application (direct) or Apple Distribution (App Store).",
            info.identity
        );
    }
    if info.role == CertRole::Unknown {
        bail!("unrecognized certificate: {}", info.common_name);
    }
    Ok(())
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

fn key_id_from_path(path: &Path) -> Result<String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if let Some(id) = stem.strip_prefix("AuthKey_") {
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    bail!(
        "could not read Key ID from {}. Name it AuthKey_<KEYID>.p8 or we cannot detect APPLE_API_KEY.",
        path.display()
    );
}

fn write_config(root: &Path, team_id: &str, force: bool) -> Result<()> {
    let path = root.join(CONFIG_FILE);
    if path.exists() && !force {
        let mut located = config::load(root)?;
        if located.config.team_id.is_empty() {
            located.config.team_id = team_id.to_string();
            config::save(root, &located.config)?;
            println!("config    updated team_id in {CONFIG_FILE}");
        } else {
            println!("config    kept existing {CONFIG_FILE}");
        }
        return Ok(());
    }
    let kind = detect::detect_kind(root)?;
    let cfg = detect::suggest_config(root, kind, team_id)?;
    config::save(root, &cfg)?;
    println!("config    wrote {CONFIG_FILE} (kind = {kind})");
    Ok(())
}

fn write_packaging_templates(root: &Path, team_id: &str) -> Result<()> {
    let dir = root.join("packaging");
    fs::create_dir_all(&dir)?;
    let did = dir.join("developer-id.entitlements");
    if !did.exists() {
        fs::write(&did, macos::default_entitlements())?;
        println!("wrote     {}", did.display());
    }
    let located = config::load(root).ok();
    let bundle_id = located
        .as_ref()
        .map(|c| c.config.bundle_id.as_str())
        .unwrap_or("com.example.app");
    let mas = dir.join("app-store.entitlements");
    if !mas.exists() {
        fs::write(
            &mas,
            macos::app_store_entitlements_template(team_id, bundle_id),
        )?;
        println!("wrote     {}", mas.display());
    }
    Ok(())
}

fn write_workflow(root: &Path, force: bool) -> Result<()> {
    let path = root.join(WORKFLOW_FILE);
    if path.exists() && !force {
        println!("workflow  kept existing {WORKFLOW_FILE}");
        return Ok(());
    }
    let kind = config::load(root)?.config.kind;
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, workflow::render(kind))?;
    println!("workflow  wrote {WORKFLOW_FILE}");
    Ok(())
}

fn push_workflow(root: &Path) -> Result<()> {
    let status = Command::new("git")
        .args(["add", CONFIG_FILE, WORKFLOW_FILE, "packaging"])
        .current_dir(root)
        .status()?;
    if !status.success() {
        bail!("git add failed");
    }
    let status = Command::new("git")
        .args(["commit", "-m", "Add apple-ship GitHub Actions workflow"])
        .current_dir(root)
        .status()?;
    if !status.success() {
        eprintln!("note      git commit skipped (nothing to commit, or commit failed)");
    }
    let status = Command::new("git")
        .args(["push"])
        .current_dir(root)
        .status()?;
    if !status.success() {
        bail!("git push failed");
    }
    println!("pushed    {WORKFLOW_FILE}");
    println!("next      apple-ship ship --channel developer-id");
    Ok(())
}
