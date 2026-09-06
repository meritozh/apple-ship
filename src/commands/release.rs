use std::env;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::{self, CONFIG_FILE};
use crate::policy::Kind;
use crate::stamp;
use crate::version::{self, SemVer};

pub fn run(spec: &str) -> Result<()> {
    let cwd = env::current_dir()?;
    let mut located = config::load(&cwd)?;
    require_clean(&located.root)?;

    hydrate_version(&located.root, &mut located.config)?;
    let current = SemVer::parse(&located.config.version)?;
    let next = current.bump(version::parse_spec(spec)?)?;
    located.config.version = next.to_string();
    located.config.build += 1;

    let mut changed = stamp::stamp(&located.root, &located.config)?;
    config::save(&located.root, &located.config)?;
    changed.push(located.root.join(CONFIG_FILE));

    let tag = format!("v{next}");
    for path in &changed {
        let rel = path.strip_prefix(&located.root).unwrap_or(path);
        git(&located.root, &["add", "--", &rel.to_string_lossy()])?;
    }
    git(&located.root, &["commit", "-m", &format!("Release {tag}")])?;
    git(&located.root, &["tag", &tag])?;
    git(&located.root, &["push", "origin", "HEAD"])?;
    git(&located.root, &["push", "origin", &tag])?;
    println!("released  {tag}  (build {})", located.config.build);
    println!("next      GitHub Actions macOS Ship should start on tag {tag}");
    Ok(())
}

fn hydrate_version(root: &std::path::Path, cfg: &mut crate::config::Config) -> Result<()> {
    if cfg.version != "0.0.0" && cfg.build != 0 {
        return Ok(());
    }
    match cfg.kind {
        Kind::Gpui => {
            let gpui = cfg.gpui()?;
            let (version, build) = stamp::read_plist_versions(&root.join(&gpui.info_plist))?;
            if cfg.version == "0.0.0" {
                cfg.version = version;
            }
            if cfg.build == 0 {
                cfg.build = build;
            }
        }
        Kind::Tauri => {
            let tauri = cfg.tauri()?;
            if cfg.version == "0.0.0" {
                cfg.version = stamp::read_tauri_version(root, &tauri.app_path)?;
            }
        }
        Kind::Native => {
            if cfg.version == "0.0.0" || cfg.build == 0 {
                let (version, build) = stamp::read_project_yml_versions(&root.join("project.yml"))?;
                if cfg.version == "0.0.0" {
                    cfg.version = version;
                }
                if cfg.build == 0 {
                    cfg.build = build;
                }
            }
        }
    }
    Ok(())
}

fn require_clean(root: &std::path::Path) -> Result<()> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .context("git status")?;
    if !output.status.success() {
        bail!("git status failed");
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if !text.trim().is_empty() {
        bail!("working tree is dirty; commit or stash before `apple-ship release`");
    }
    Ok(())
}

fn git(root: &std::path::Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .with_context(|| format!("git {}", args.join(" ")))?;
    if !status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(())
}
