use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::policy::Kind;

pub const CONFIG_FILE: &str = "apple-ship.toml";
pub const WORKFLOW_FILE: &str = ".github/workflows/macos-ship.yml";
pub const RELEASE_ENV: &str = "release";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub kind: Kind,
    pub team_id: String,
    pub bundle_id: String,
    pub product_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpui: Option<GpuiConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tauri: Option<TauriConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GpuiConfig {
    pub bin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    pub info_plist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entitlements: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TauriConfig {
    #[serde(default = "default_tauri_app_path")]
    pub app_path: String,
}

fn default_tauri_app_path() -> String {
    "src-tauri".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub scheme: String,
}

#[derive(Clone, Debug)]
pub struct LocatedConfig {
    pub root: PathBuf,
    pub path: PathBuf,
    pub config: Config,
}

pub fn find_root(start: &Path) -> Result<PathBuf> {
    let mut dir = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    loop {
        if dir.join(CONFIG_FILE).is_file() || dir.join(".git").exists() {
            return Ok(dir);
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => anyhow::bail!(
                "could not find {CONFIG_FILE} or a git root above {}",
                start.display()
            ),
        }
    }
}

pub fn load(start: &Path) -> Result<LocatedConfig> {
    let root = find_root(start)?;
    let path = root.join(CONFIG_FILE);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("missing {CONFIG_FILE} at {}", path.display()))?;
    let config: Config = toml::from_str(&text)
        .with_context(|| format!("invalid {CONFIG_FILE} at {}", path.display()))?;
    Ok(LocatedConfig { root, path, config })
}

pub fn save(root: &Path, config: &Config) -> Result<PathBuf> {
    let path = root.join(CONFIG_FILE);
    let mut text = toml::to_string_pretty(config)?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    fs::write(&path, text)?;
    Ok(path)
}

impl Config {
    pub fn gpui(&self) -> Result<&GpuiConfig> {
        self.gpui
            .as_ref()
            .context("apple-ship.toml is missing [gpui]; kind = \"gpui\" requires it")
    }

    pub fn tauri(&self) -> Result<&TauriConfig> {
        self.tauri
            .as_ref()
            .context("apple-ship.toml is missing [tauri]; kind = \"tauri\" requires it")
    }

    pub fn native(&self) -> Result<&NativeConfig> {
        self.native
            .as_ref()
            .context("apple-ship.toml is missing [native]; kind = \"native\" requires it")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_gpui_config() {
        let cfg = Config {
            kind: Kind::Gpui,
            team_id: "N59353RP3W".into(),
            bundle_id: "com.daggy.app".into(),
            product_name: "daggy".into(),
            gpui: Some(GpuiConfig {
                bin: "daggy".into(),
                package: Some("daggy-app".into()),
                info_plist: "crates/daggy-app/Info.plist".into(),
                icon: Some("crates/daggy-app/Assets/AppIcon.icns".into()),
                entitlements: None,
            }),
            tauri: None,
            native: None,
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.kind, Kind::Gpui);
        assert_eq!(back.team_id, "N59353RP3W");
        assert_eq!(back.gpui.unwrap().bin, "daggy");
    }
}
