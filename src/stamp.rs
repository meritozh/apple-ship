use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::Config;
use crate::policy::Kind;

pub fn stamp(root: &Path, config: &Config) -> Result<Vec<PathBuf>> {
    match config.kind {
        Kind::Gpui => stamp_gpui(root, config),
        Kind::Tauri => stamp_tauri(root, config),
        Kind::Native => stamp_native(root, config),
    }
}

fn stamp_gpui(root: &Path, config: &Config) -> Result<Vec<PathBuf>> {
    let gpui = config.gpui()?;
    let mut changed = Vec::new();
    let plist_path = root.join(&gpui.info_plist);
    write_plist_versions(&plist_path, &config.version, config.build)?;
    changed.push(plist_path);
    let cargo = root.join("Cargo.toml");
    if cargo.is_file() {
        let text = fs::read_to_string(&cargo)?;
        let updated = set_cargo_version(&text, &config.version)?;
        if updated != text {
            fs::write(&cargo, updated)?;
            changed.push(cargo);
        }
    }
    Ok(changed)
}

fn stamp_tauri(root: &Path, config: &Config) -> Result<Vec<PathBuf>> {
    let tauri = config.tauri()?;
    let mut changed = Vec::new();
    for rel in [
        format!("{}/tauri.conf.json", tauri.app_path),
        format!("{}/tauri.conf.json5", tauri.app_path),
    ] {
        let path = root.join(&rel);
        if path.is_file() {
            set_json_version(&path, &config.version)?;
            changed.push(path);
            break;
        }
    }
    if changed.is_empty() {
        bail!("missing tauri.conf.json under {}", tauri.app_path);
    }
    let pkg = root.join("package.json");
    if pkg.is_file() {
        set_json_version(&pkg, &config.version)?;
        changed.push(pkg);
    }
    let cargo = root.join(&tauri.app_path).join("Cargo.toml");
    if cargo.is_file() {
        let text = fs::read_to_string(&cargo)?;
        let updated = set_cargo_version(&text, &config.version)?;
        if updated != text {
            fs::write(&cargo, updated)?;
            changed.push(cargo);
        }
    }
    Ok(changed)
}

fn stamp_native(root: &Path, config: &Config) -> Result<Vec<PathBuf>> {
    let yml = root.join("project.yml");
    if !yml.is_file() {
        bail!("native version stamping requires project.yml (XcodeGen)");
    }
    let text = fs::read_to_string(&yml)?;
    let mut updated = set_yaml_scalar(&text, "MARKETING_VERSION", &config.version)?;
    updated = set_yaml_scalar(
        &updated,
        "CURRENT_PROJECT_VERSION",
        &config.build.to_string(),
    )?;
    fs::write(&yml, updated)?;
    Ok(vec![yml])
}

fn write_plist_versions(path: &Path, version: &str, build: u64) -> Result<()> {
    let mut value =
        plist::Value::from_file(path).with_context(|| format!("read {}", path.display()))?;
    let dict = value
        .as_dictionary_mut()
        .context("Info.plist root is not a dict")?;
    dict.insert(
        "CFBundleShortVersionString".into(),
        plist::Value::String(version.to_string()),
    );
    dict.insert(
        "CFBundleVersion".into(),
        plist::Value::String(build.to_string()),
    );
    value
        .to_file_xml(path)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn set_cargo_version(text: &str, version: &str) -> Result<String> {
    for section in ["[workspace.package]", "[package]"] {
        if let Some(updated) = replace_in_section(text, section, "version", version) {
            return Ok(updated);
        }
    }
    bail!("Cargo.toml has no [workspace.package] or [package] version field")
}

fn replace_in_section(text: &str, header: &str, key: &str, value: &str) -> Option<String> {
    let start = text.find(header)?;
    let after = start + header.len();
    let end = text[after..]
        .find("\n[")
        .map(|i| after + i)
        .unwrap_or(text.len());
    let section = &text[start..end];
    let pattern = format!("{key} = \"");
    let rel = section.find(&pattern)?;
    let value_start = rel + pattern.len();
    let rest = &section[value_start..];
    let value_end = rest.find('"')?;
    let abs_start = start + value_start;
    let abs_end = abs_start + value_end;
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..abs_start]);
    out.push_str(value);
    out.push_str(&text[abs_end..]);
    Some(out)
}

fn set_json_version(path: &Path, version: &str) -> Result<()> {
    let text = fs::read_to_string(path)?;
    let mut v: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("invalid JSON {}", path.display()))?;
    let obj = v
        .as_object_mut()
        .with_context(|| format!("{} root is not an object", path.display()))?;
    obj.insert("version".into(), serde_json::Value::String(version.into()));
    let mut out = serde_json::to_string_pretty(&v)?;
    out.push('\n');
    fs::write(path, out)?;
    Ok(())
}

fn set_yaml_scalar(text: &str, key: &str, value: &str) -> Result<String> {
    let needle = format!("{key}:");
    let mut found = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(&needle) {
            let indent_len = line.len() - trimmed.len();
            let indent = &line[..indent_len];
            let quote = if rest.trim().starts_with('"') {
                "\""
            } else {
                ""
            };
            lines.push(format!("{indent}{key}: {quote}{value}{quote}"));
            found = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !found {
        bail!("project.yml has no {key}");
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

pub fn read_tauri_version(root: &Path, app_path: &str) -> Result<String> {
    for name in ["tauri.conf.json", "tauri.conf.json5"] {
        let path = root.join(app_path).join(name);
        if path.is_file() {
            let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)
                .with_context(|| format!("invalid JSON {}", path.display()))?;
            return v
                .get("version")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .with_context(|| format!("{} missing version", path.display()));
        }
    }
    bail!("missing tauri.conf.json under {app_path}")
}

pub fn read_project_yml_versions(path: &Path) -> Result<(String, u64)> {
    let text = fs::read_to_string(path).with_context(|| format!("missing {}", path.display()))?;
    let version =
        yaml_value(&text, "MARKETING_VERSION").context("project.yml missing MARKETING_VERSION")?;
    let build = yaml_value(&text, "CURRENT_PROJECT_VERSION")
        .context("project.yml missing CURRENT_PROJECT_VERSION")?
        .parse::<u64>()
        .context("CURRENT_PROJECT_VERSION is not an integer")?;
    Ok((version, build))
}

fn yaml_value(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(&prefix) {
            let v = rest.trim().trim_matches('"');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

pub fn read_plist_versions(path: &Path) -> Result<(String, u64)> {
    let value = plist::Value::from_file(path)?;
    let dict = value
        .as_dictionary()
        .context("Info.plist root is not a dict")?;
    let version = dict
        .get("CFBundleShortVersionString")
        .and_then(|v| v.as_string())
        .context("Info.plist missing CFBundleShortVersionString")?
        .to_string();
    let build = dict
        .get("CFBundleVersion")
        .and_then(|v| v.as_string())
        .context("Info.plist missing CFBundleVersion")?
        .parse::<u64>()
        .context("CFBundleVersion is not an integer")?;
    Ok((version, build))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_workspace_package_version() {
        let text = "[workspace]\n\n[workspace.package]\nversion = \"1.0.1\"\nedition = \"2021\"\n\n[dependencies]\nrkyv = { version = \"0.8\" }\n";
        let out = set_cargo_version(text, "1.0.2").unwrap();
        assert!(out.contains("version = \"1.0.2\""));
        assert!(out.contains("rkyv = { version = \"0.8\" }"));
        assert!(out.contains("edition = \"2021\""));
    }

    #[test]
    fn yaml_marketing_version() {
        let text = "settings:\n  base:\n    MARKETING_VERSION: \"1.1.2\"\n    CURRENT_PROJECT_VERSION: \"4\"\n";
        let out = set_yaml_scalar(text, "MARKETING_VERSION", "1.1.3").unwrap();
        let out = set_yaml_scalar(&out, "CURRENT_PROJECT_VERSION", "5").unwrap();
        assert!(out.contains("MARKETING_VERSION: \"1.1.3\""));
        assert!(out.contains("CURRENT_PROJECT_VERSION: \"5\""));
    }
}
