use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::config::{Config, GpuiConfig, NativeConfig, TauriConfig};
use crate::policy::{Channel, Kind};

pub fn detect_kind(root: &Path) -> Result<Kind> {
    let mut kinds = Vec::new();
    if has_tauri(root) {
        kinds.push(Kind::Tauri);
    }
    if has_native(root) {
        kinds.push(Kind::Native);
    }
    if has_gpui(root) {
        kinds.push(Kind::Gpui);
    }
    match kinds.as_slice() {
        [kind] => Ok(*kind),
        [] => bail!(
            "could not detect app kind in {}. Pass --kind gpui|tauri|native.",
            root.display()
        ),
        many => bail!(
            "ambiguous app kind in {}: {}. Pass --kind gpui|tauri|native.",
            root.display(),
            many.iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub fn suggest_config(root: &Path, kind: Kind, team_id: &str) -> Result<Config> {
    match kind {
        Kind::Gpui => suggest_gpui(root, team_id),
        Kind::Tauri => suggest_tauri(root, team_id),
        Kind::Native => suggest_native(root, team_id),
    }
}

pub fn prepare_for_setup(
    root: &Path,
    existing: Option<Config>,
    kind_override: Option<Kind>,
    channel: Channel,
    team_id: &str,
) -> Result<Config> {
    let mut cfg = match (existing, kind_override) {
        (None, kind) => {
            let kind = match kind {
                Some(k) => k,
                None => detect_kind(root)?,
            };
            suggest_config(root, kind, team_id)?
        }
        (Some(cfg), None) => {
            if cfg.team_id != team_id {
                bail!(
                    "apple-ship.toml team_id is {} but this certificate is team {}",
                    cfg.team_id,
                    team_id
                );
            }
            cfg
        }
        (Some(cfg), Some(kind)) => {
            if cfg.team_id != team_id {
                bail!(
                    "apple-ship.toml team_id is {} but this certificate is team {}",
                    cfg.team_id,
                    team_id
                );
            }
            if cfg.kind == kind {
                cfg
            } else {
                let mut generated = suggest_config(root, kind, team_id)?;
                generated.channels = cfg.channels;
                generated
            }
        }
    };
    if !cfg.channels.contains(&channel) {
        cfg.channels.push(channel);
        cfg.channels.sort_by_key(|c| match c {
            Channel::DeveloperId => 0,
            Channel::AppStore => 1,
        });
    }
    Ok(cfg)
}

fn has_tauri(root: &Path) -> bool {
    root.join("src-tauri/tauri.conf.json").is_file()
        || root.join("src-tauri/tauri.conf.json5").is_file()
}

fn has_native(root: &Path) -> bool {
    if root.join("project.yml").is_file() {
        return true;
    }
    glob_exists(root, "xcodeproj") || glob_exists(root, "xcworkspace")
}

fn has_gpui(root: &Path) -> bool {
    cargo_mentions_gpui(&root.join("Cargo.toml"))
        || walk_cargo_tomls(root, 3)
            .iter()
            .any(|p| cargo_mentions_gpui(p))
}

fn cargo_mentions_gpui(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|text| {
            text.lines().any(|line| {
                let line = line.trim();
                !line.starts_with('#') && (line.starts_with("gpui") || line.contains("gpui ="))
            })
        })
        .unwrap_or(false)
}

fn glob_exists(root: &Path, ext: &str) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|e| {
        e.path()
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| x == ext)
    })
}

fn skip_dir(name: &std::ffi::OsStr) -> bool {
    name == "target"
        || name == ".git"
        || name == ".delta"
        || name == "node_modules"
        || name == "build"
        || name == "Pods"
        || name.to_str().is_some_and(|s| s.ends_with(".app"))
}

fn walk_cargo_tomls(root: &Path, depth: usize) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    fn rec(dir: &Path, depth: usize, out: &mut Vec<std::path::PathBuf>) {
        if depth == 0 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if skip_dir(&name) {
                continue;
            }
            if path.is_dir() {
                rec(&path, depth - 1, out);
            } else if name == "Cargo.toml" {
                out.push(path);
            }
        }
    }
    rec(root, depth, &mut out);
    out
}

fn suggest_gpui(root: &Path, team_id: &str) -> Result<Config> {
    let plist = find_info_plist(root)?;
    let meta = read_plist_identity(root, &plist)?;
    let bin = meta
        .executable
        .clone()
        .unwrap_or_else(|| meta.product_name.clone());
    let package = infer_package_name(root);
    let icon = find_icns(root)?;
    Ok(Config {
        kind: Kind::Gpui,
        team_id: team_id.to_string(),
        bundle_id: meta.bundle_id,
        product_name: meta.product_name,
        version: meta.version,
        build: meta.build,
        channels: Vec::new(),
        gpui: Some(GpuiConfig {
            bin,
            package,
            info_plist: rel(root, &plist),
            icon: icon.map(|p| rel(root, &p)),
            entitlements: None,
        }),
        tauri: None,
        native: None,
    })
}

fn suggest_tauri(root: &Path, team_id: &str) -> Result<Config> {
    let conf_path = ["src-tauri/tauri.conf.json", "src-tauri/tauri.conf.json5"]
        .into_iter()
        .map(|p| root.join(p))
        .find(|p| p.is_file())
        .context("missing src-tauri/tauri.conf.json")?;
    let text = fs::read_to_string(&conf_path)?;
    let v: serde_json::Value = serde_json::from_str(&text).context("invalid tauri.conf.json")?;
    let bundle_id = v
        .get("identifier")
        .and_then(|x| x.as_str())
        .unwrap_or("com.example.app")
        .to_string();
    let product_name = v
        .get("productName")
        .and_then(|x| x.as_str())
        .unwrap_or("App")
        .to_string();
    let version = v
        .get("version")
        .and_then(|x| x.as_str())
        .unwrap_or("0.0.0")
        .to_string();
    Ok(Config {
        kind: Kind::Tauri,
        team_id: team_id.to_string(),
        bundle_id,
        product_name,
        version,
        build: 0,
        channels: Vec::new(),
        gpui: None,
        tauri: Some(TauriConfig {
            app_path: "src-tauri".into(),
        }),
        native: None,
    })
}

fn suggest_native(root: &Path, team_id: &str) -> Result<Config> {
    let mut bundle_id = "com.example.app".to_string();
    let mut product_name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("App")
        .to_string();
    let mut scheme = product_name.clone();
    let mut project = None;
    let mut workspace = None;
    let mut version = "0.0.0".to_string();
    let mut build = 0u64;

    let yml = root.join("project.yml");
    if yml.is_file() {
        let text = fs::read_to_string(&yml)?;
        if let Some(name) = yaml_scalar(&text, "name") {
            product_name = name.clone();
            scheme = name;
        }
        if let Some(prefix) = yaml_scalar(&text, "bundleIdPrefix") {
            bundle_id = format!("{prefix}.{}", product_name.to_ascii_lowercase());
        }
        if let Some(id) = find_line_value(&text, "PRODUCT_BUNDLE_IDENTIFIER:") {
            bundle_id = id.trim_matches('"').to_string();
        }
        if let Some(v) = find_line_value(&text, "MARKETING_VERSION:") {
            version = v.trim_matches('"').to_string();
        }
        if let Some(b) = find_line_value(&text, "CURRENT_PROJECT_VERSION:") {
            if let Ok(n) = b.trim_matches('"').parse::<u64>() {
                build = n;
            }
        }
    }

    if let Some(ws) = first_with_ext(root, "xcworkspace") {
        if !ws
            .to_string_lossy()
            .contains(".xcodeproj/project.xcworkspace")
        {
            workspace = Some(rel(root, &ws));
        }
    }
    if let Some(proj) = first_with_ext(root, "xcodeproj") {
        project = Some(rel(root, &proj));
        if scheme == product_name {
            if let Some(stem) = proj.file_stem().and_then(|s| s.to_str()) {
                if scheme == root.file_name().and_then(|n| n.to_str()).unwrap_or("") {
                    scheme = stem.to_string();
                    product_name = stem.to_string();
                }
            }
        }
    }

    Ok(Config {
        kind: Kind::Native,
        team_id: team_id.to_string(),
        bundle_id,
        product_name,
        version,
        build,
        channels: Vec::new(),
        gpui: None,
        tauri: None,
        native: Some(NativeConfig {
            project,
            workspace,
            scheme,
        }),
    })
}

struct PlistMeta {
    bundle_id: String,
    product_name: String,
    executable: Option<String>,
    version: String,
    build: u64,
}

fn read_plist_identity(root: &Path, plist_path: &Path) -> Result<PlistMeta> {
    let value = plist::Value::from_file(plist_path)
        .with_context(|| format!("invalid Info.plist {}", plist_path.display()))?;
    let dict = value
        .as_dictionary()
        .context("Info.plist root is not a dict")?;
    let bundle_id = dict
        .get("CFBundleIdentifier")
        .and_then(|v| v.as_string())
        .unwrap_or("com.example.app")
        .to_string();
    let product_name = dict
        .get("CFBundleDisplayName")
        .or_else(|| dict.get("CFBundleName"))
        .and_then(|v| v.as_string())
        .unwrap_or("App")
        .to_string();
    let executable = dict
        .get("CFBundleExecutable")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string());
    let version = dict
        .get("CFBundleShortVersionString")
        .and_then(|v| v.as_string())
        .unwrap_or("0.0.0")
        .to_string();
    let build = dict
        .get("CFBundleVersion")
        .and_then(|v| v.as_string())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let _ = root;
    Ok(PlistMeta {
        bundle_id,
        product_name,
        executable,
        version,
        build,
    })
}

fn find_info_plist(root: &Path) -> Result<std::path::PathBuf> {
    let mut found = Vec::new();
    collect_named(root, 5, "Info.plist", &mut found);
    unique_source(root, "Info.plist", found)
}

fn find_icns(root: &Path) -> Result<Option<std::path::PathBuf>> {
    let mut found = Vec::new();
    fn rec(dir: &Path, depth: usize, found: &mut Vec<std::path::PathBuf>) {
        if depth == 0 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if skip_dir(&name) {
                continue;
            }
            if path.is_dir() {
                rec(&path, depth - 1, found);
            } else if path.extension().and_then(|e| e.to_str()) == Some("icns") {
                found.push(path);
            }
        }
    }
    rec(root, 5, &mut found);
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(found.remove(0))),
        _ => bail!(
            "multiple .icns files: {}. Set [gpui].icon in apple-ship.toml",
            found
                .iter()
                .map(|p| rel(root, p))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn collect_named(root: &Path, depth: usize, filename: &str, found: &mut Vec<std::path::PathBuf>) {
    fn rec(dir: &Path, depth: usize, filename: &str, found: &mut Vec<std::path::PathBuf>) {
        if depth == 0 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if skip_dir(&name) {
                continue;
            }
            if path.is_dir() {
                rec(&path, depth - 1, filename, found);
            } else if name == filename {
                found.push(path);
            }
        }
    }
    rec(root, depth, filename, found);
}

fn unique_source(
    root: &Path,
    label: &str,
    found: Vec<std::path::PathBuf>,
) -> Result<std::path::PathBuf> {
    match found.as_slice() {
        [] => bail!("could not find {label}"),
        [one] => Ok(one.clone()),
        _ => bail!(
            "multiple {label} files: {}. Set the path in apple-ship.toml",
            found
                .iter()
                .map(|p| rel(root, p))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn infer_package_name(root: &Path) -> Option<String> {
    for cargo in walk_cargo_tomls(root, 4) {
        if cargo == root.join("Cargo.toml") {
            continue;
        }
        if cargo_mentions_gpui(&cargo) {
            if let Ok(text) = fs::read_to_string(&cargo) {
                if let Some(name) = cargo_package_name(&text) {
                    return Some(name);
                }
            }
        }
    }
    None
}

fn cargo_package_name(text: &str) -> Option<String> {
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = line.strip_prefix("name") {
                if let Some(val) = rest.split('=').nth(1) {
                    return Some(val.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    None
}

fn first_with_ext(root: &Path, ext: &str) -> Option<std::path::PathBuf> {
    fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
}

fn yaml_scalar(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim().trim_start_matches(':').trim();
            if !rest.is_empty() && !rest.starts_with('\n') {
                return Some(rest.trim_matches('"').to_string());
            }
        }
    }
    None
}

fn find_line_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix(key)
            .map(|v| v.trim().trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
    })
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn detects_tauri() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src-tauri")).unwrap();
        fs::write(
            dir.path().join("src-tauri/tauri.conf.json"),
            r#"{"productName":"Climber","identifier":"com.gaowanqiu.climber"}"#,
        )
        .unwrap();
        assert_eq!(detect_kind(dir.path()).unwrap(), Kind::Tauri);
        let cfg = suggest_tauri(dir.path(), "N59353RP3W").unwrap();
        assert_eq!(cfg.bundle_id, "com.gaowanqiu.climber");
        assert_eq!(cfg.product_name, "Climber");
    }

    #[test]
    fn detects_gpui_from_cargo_toml() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n\n[dependencies]\ngpui = \"0.2\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.demo.app</string>
<key>CFBundleName</key><string>demo</string>
<key>CFBundleExecutable</key><string>demo</string>
</dict></plist>"#,
        )
        .unwrap();
        assert_eq!(detect_kind(dir.path()).unwrap(), Kind::Gpui);
        let cfg = suggest_gpui(dir.path(), "N59353RP3W").unwrap();
        assert_eq!(cfg.bundle_id, "com.demo.app");
        assert_eq!(cfg.gpui.unwrap().bin, "demo");
    }

    #[test]
    fn ignores_delta_worktrees_and_app_bundles() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n\n[dependencies]\ngpui = \"0.2\"\n",
        )
        .unwrap();
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.demo.app</string>
<key>CFBundleName</key><string>demo</string>
<key>CFBundleExecutable</key><string>demo</string>
</dict></plist>"#;
        fs::create_dir_all(dir.path().join("crates/demo")).unwrap();
        fs::write(dir.path().join("crates/demo/Info.plist"), plist).unwrap();
        fs::create_dir_all(dir.path().join(".delta/worktrees/x/crates/demo")).unwrap();
        fs::write(
            dir.path().join(".delta/worktrees/x/crates/demo/Info.plist"),
            plist.replace("com.demo.app", "com.wrong.app"),
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("target/release/demo.app/Contents")).unwrap();
        fs::write(
            dir.path()
                .join("target/release/demo.app/Contents/Info.plist"),
            plist.replace("com.demo.app", "com.bundle.app"),
        )
        .unwrap();
        let cfg = suggest_gpui(dir.path(), "N59353RP3W").unwrap();
        assert_eq!(cfg.bundle_id, "com.demo.app");
        assert_eq!(cfg.gpui.unwrap().info_plist, "crates/demo/Info.plist");
    }

    #[test]
    fn detects_native_from_xcodegen() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("project.yml"),
            "name: Sotto\noptions:\n  bundleIdPrefix: com.meritozh\n",
        )
        .unwrap();
        assert_eq!(detect_kind(dir.path()).unwrap(), Kind::Native);
        let cfg = suggest_native(dir.path(), "N59353RP3W").unwrap();
        assert_eq!(cfg.product_name, "Sotto");
        assert!(cfg.bundle_id.contains("sotto"));
    }

    fn tauri_fixture() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src-tauri")).unwrap();
        fs::write(
            dir.path().join("src-tauri/tauri.conf.json"),
            r#"{"productName":"Climber","identifier":"com.gaowanqiu.climber"}"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn setup_generates_toml_from_detect() {
        let dir = tauri_fixture();
        let cfg =
            prepare_for_setup(dir.path(), None, None, Channel::DeveloperId, "N59353RP3W").unwrap();
        assert_eq!(cfg.kind, Kind::Tauri);
        assert_eq!(cfg.bundle_id, "com.gaowanqiu.climber");
        assert_eq!(cfg.channels, vec![Channel::DeveloperId]);
    }

    #[test]
    fn setup_updates_existing_channels() {
        let dir = tauri_fixture();
        let first =
            prepare_for_setup(dir.path(), None, None, Channel::DeveloperId, "N59353RP3W").unwrap();
        let second = prepare_for_setup(
            dir.path(),
            Some(first),
            None,
            Channel::AppStore,
            "N59353RP3W",
        )
        .unwrap();
        assert_eq!(second.kind, Kind::Tauri);
        assert_eq!(
            second.channels,
            vec![Channel::DeveloperId, Channel::AppStore]
        );
    }

    #[test]
    fn setup_kind_override_skips_detect() {
        let dir = tauri_fixture();
        fs::write(
            dir.path().join("project.yml"),
            "name: Sotto\noptions:\n  bundleIdPrefix: com.meritozh\n",
        )
        .unwrap();
        let err = detect_kind(dir.path()).unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
        let cfg = prepare_for_setup(
            dir.path(),
            None,
            Some(Kind::Tauri),
            Channel::DeveloperId,
            "N59353RP3W",
        )
        .unwrap();
        assert_eq!(cfg.kind, Kind::Tauri);
        assert_eq!(cfg.product_name, "Climber");
    }

    #[test]
    fn setup_rejects_team_mismatch_on_update() {
        let dir = tauri_fixture();
        let first =
            prepare_for_setup(dir.path(), None, None, Channel::DeveloperId, "N59353RP3W").unwrap();
        let err = prepare_for_setup(
            dir.path(),
            Some(first),
            None,
            Channel::DeveloperId,
            "AAAAAAAAAA",
        )
        .unwrap_err();
        assert!(err.to_string().contains("team"));
    }
}
