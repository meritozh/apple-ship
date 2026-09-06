use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use base64::Engine;

use crate::policy::Channel;

pub fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

pub fn require_github_actions() -> Result<()> {
    if env_nonempty("GITHUB_ACTIONS").as_deref() != Some("true") {
        bail!(
            "apple-ship ci only runs on GitHub Actions. From your machine, use `apple-ship ship --channel ...`."
        );
    }
    Ok(())
}

pub fn import_certificate_p12(p12_b64: &str, password: &str) -> Result<PathBuf> {
    let tmp = std::env::var("RUNNER_TEMP")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let p12_path = tmp.join("apple-ship-cert.p12");
    let keychain = tmp.join("apple-ship.keychain-db");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(p12_b64.trim().as_bytes())
        .or_else(|_| {
            let stripped: String = p12_b64.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD.decode(stripped.as_bytes())
        })
        .context("APPLE_CERTIFICATE is not valid base64")?;
    fs::write(&p12_path, bytes)?;

    let keychain_password =
        env_nonempty("KEYCHAIN_PASSWORD").unwrap_or_else(|| format!("ks-{}", std::process::id()));
    if keychain.exists() {
        let _ = run_allow_fail(
            "security",
            &["delete-keychain", &keychain.to_string_lossy()],
        );
    }
    run(
        "security",
        &[
            "create-keychain",
            "-p",
            &keychain_password,
            &keychain.to_string_lossy(),
        ],
    )?;
    run(
        "security",
        &[
            "set-keychain-settings",
            "-lut",
            "21600",
            &keychain.to_string_lossy(),
        ],
    )?;
    run(
        "security",
        &[
            "unlock-keychain",
            "-p",
            &keychain_password,
            &keychain.to_string_lossy(),
        ],
    )?;
    run(
        "security",
        &[
            "import",
            &p12_path.to_string_lossy(),
            "-k",
            &keychain.to_string_lossy(),
            "-P",
            password,
            "-A",
            "-T",
            "/usr/bin/codesign",
            "-f",
            "pkcs12",
        ],
    )?;
    run(
        "security",
        &[
            "set-key-partition-list",
            "-S",
            "apple-tool:,apple:,codesign:",
            "-s",
            "-k",
            &keychain_password,
            &keychain.to_string_lossy(),
        ],
    )?;
    run(
        "security",
        &[
            "list-keychain",
            "-d",
            "user",
            "-s",
            &keychain.to_string_lossy(),
        ],
    )?;
    let _ = fs::remove_file(&p12_path);
    Ok(keychain)
}

pub fn find_identity(channel: Channel, team_id: &str) -> Result<String> {
    if let Some(explicit) = env_nonempty("APPLE_SIGNING_IDENTITY") {
        return Ok(explicit);
    }
    let needle = match channel {
        Channel::DeveloperId => "Developer ID Application:",
        Channel::AppStore => "Apple Distribution:",
    };
    let output = Command::new("security")
        .args(["find-identity", "-v", "-p", "codesigning"])
        .output()
        .context("security find-identity")?;
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if !line.contains(needle) || !line.contains(team_id) {
            continue;
        }
        if let Some(start) = line.find('"') {
            if let Some(end) = line.rfind('"') {
                if end > start {
                    return Ok(line[start + 1..end].to_string());
                }
            }
        }
    }
    bail!("no {needle} identity for team {team_id} in the keychain");
}

pub fn codesign_path(
    identity: &str,
    path: &Path,
    entitlements: Option<&Path>,
    hardened: bool,
) -> Result<()> {
    let mut args = vec![
        "--force".to_string(),
        "--sign".to_string(),
        identity.to_string(),
        "--timestamp".to_string(),
    ];
    if hardened {
        args.push("--options".into());
        args.push("runtime".into());
    }
    if let Some(ent) = entitlements {
        args.push("--entitlements".into());
        args.push(ent.to_string_lossy().into_owned());
    }
    args.push(path.to_string_lossy().into_owned());
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run("codesign", &args_ref)?;
    run(
        "codesign",
        &[
            "--verify",
            "--strict",
            "--verbose=2",
            &path.to_string_lossy(),
        ],
    )?;
    Ok(())
}

pub fn create_dmg(app: &Path, dmg: &Path, volume_name: &str) -> Result<()> {
    if dmg.exists() {
        fs::remove_file(dmg)?;
    }
    run(
        "hdiutil",
        &[
            "create",
            "-volname",
            volume_name,
            "-srcfolder",
            &app.to_string_lossy(),
            "-ov",
            "-format",
            "UDZO",
            &dmg.to_string_lossy(),
        ],
    )?;
    Ok(())
}

pub fn notarize(dmg: &Path, team_id: &str) -> Result<()> {
    let mut args = vec![
        "notarytool".to_string(),
        "submit".to_string(),
        dmg.to_string_lossy().into_owned(),
        "--wait".to_string(),
    ];
    if let (Some(key_id), Some(issuer), Some(p8)) = (
        env_nonempty("APPLE_API_KEY"),
        env_nonempty("APPLE_API_ISSUER"),
        env_nonempty("APPLE_API_KEY_P8"),
    ) {
        let tmp = std::env::var("RUNNER_TEMP")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        let key_path = tmp.join("AuthKey.p8");
        fs::write(&key_path, p8)?;
        args.extend([
            "--key".into(),
            key_path.to_string_lossy().into_owned(),
            "--key-id".into(),
            key_id,
            "--issuer".into(),
            issuer,
        ]);
    } else if let (Some(apple_id), Some(password)) = (
        env_nonempty("APPLE_ID"),
        env_nonempty("APPLE_APP_SPECIFIC_PASSWORD").or_else(|| env_nonempty("APPLE_PASSWORD")),
    ) {
        args.extend([
            "--apple-id".into(),
            apple_id,
            "--password".into(),
            password,
            "--team-id".into(),
            team_id.to_string(),
        ]);
    } else {
        bail!(
            "notarization credentials missing. Set APPLE_ID + APPLE_APP_SPECIFIC_PASSWORD, or APPLE_API_KEY + APPLE_API_ISSUER + APPLE_API_KEY_P8."
        );
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run("xcrun", &args_ref)?;
    Ok(())
}

pub fn staple(path: &Path) -> Result<()> {
    run("xcrun", &["stapler", "staple", &path.to_string_lossy()])?;
    Ok(())
}

pub fn write_github_output(key: &str, value: &str) -> Result<()> {
    println!("{key}={value}");
    if let Some(path) = env_nonempty("GITHUB_OUTPUT") {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(file, "{key}={value}")?;
    }
    Ok(())
}

pub fn run(cmd: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("failed to spawn {cmd}"))?;
    if !output.status.success() {
        bail!(
            "{cmd} {} failed: {}",
            args.iter()
                .filter(|a| !looks_secret(a))
                .cloned()
                .collect::<Vec<_>>()
                .join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run_allow_fail(cmd: &str, args: &[&str]) -> Result<()> {
    let _ = Command::new(cmd).args(args).output();
    Ok(())
}

fn looks_secret(arg: &str) -> bool {
    arg.len() > 24 && !arg.starts_with('-') && !arg.contains('/') && !arg.contains('.')
}

pub fn newest_with_extension(root: &Path, ext: &str) -> Result<PathBuf> {
    let mut matches = Vec::new();
    fn rec(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name == ".git" || name == "node_modules" {
                continue;
            }
            if path.is_dir() {
                rec(&path, ext, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                out.push(path);
            }
        }
    }
    rec(root, ext, &mut matches);
    matches.sort_by_key(|p| {
        fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    matches
        .pop()
        .with_context(|| format!("no .{ext} found under {}", root.display()))
}

pub fn default_entitlements() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
</dict>
</plist>
"#
}

pub fn app_store_entitlements_template(team_id: &str, bundle_id: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.app-sandbox</key>
    <true/>
    <key>com.apple.application-identifier</key>
    <string>{team_id}.{bundle_id}</string>
    <key>com.apple.developer.team-identifier</key>
    <string>{team_id}</string>
</dict>
</plist>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_env_is_none() {
        assert_eq!(env_nonempty("APPLE_SHIP_TEST_UNSET_VAR_XYZ"), None);
    }
}
