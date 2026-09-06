use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::{Config, NativeConfig};
use crate::macos::{self, codesign_path, create_dmg, find_identity, notarize, staple};
use crate::policy::{Channel, Kind};

pub fn ship_developer_id(root: &Path, config: &Config) -> Result<PathBuf> {
    match config.kind {
        Kind::Gpui => ship_gpui(root, config),
        Kind::Tauri => ship_tauri(root, config),
        Kind::Native => ship_native(root, config),
    }
}

fn ship_gpui(root: &Path, config: &Config) -> Result<PathBuf> {
    let gpui = config.gpui()?;
    let mut build = Command::new("cargo");
    build.arg("build").arg("--release").current_dir(root);
    if let Some(package) = &gpui.package {
        build.arg("-p").arg(package);
    }
    build.arg("--bin").arg(&gpui.bin);
    let status = build.status().context("cargo build")?;
    if !status.success() {
        bail!("cargo build --release failed");
    }
    let bin_path = root.join("target/release").join(&gpui.bin);
    if !bin_path.is_file() {
        bail!("release binary not found at {}", bin_path.display());
    }

    let out_dir = root.join("target/apple-ship");
    fs::create_dir_all(&out_dir)?;
    let app = out_dir.join(format!("{}.app", config.product_name));
    assemble_gpui_app(root, config, &bin_path, &app)?;

    let identity = find_identity(Channel::DeveloperId, &config.team_id)?;
    let entitlements = entitlements_path(root, gpui.entitlements.as_deref())?;
    let executable = executable_name(&root.join(&gpui.info_plist))?;
    codesign_path(
        &identity,
        &app.join("Contents/MacOS").join(&executable),
        Some(&entitlements),
        true,
    )?;
    codesign_path(&identity, &app, Some(&entitlements), true)?;

    let dmg = out_dir.join(format!("{}.dmg", config.product_name));
    create_dmg(&app, &dmg, &config.product_name)?;
    codesign_path(&identity, &dmg, None, false)?;
    notarize(&dmg, &config.team_id)?;
    staple(&dmg)?;
    Ok(dmg)
}

fn assemble_gpui_app(root: &Path, config: &Config, bin: &Path, dest: &Path) -> Result<()> {
    let gpui = config.gpui()?;
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    let macos = dest.join("Contents/MacOS");
    let resources = dest.join("Contents/Resources");
    fs::create_dir_all(&macos)?;
    fs::create_dir_all(&resources)?;
    let plist_src = root.join(&gpui.info_plist);
    fs::copy(&plist_src, dest.join("Contents/Info.plist"))
        .with_context(|| format!("copy {}", plist_src.display()))?;
    let executable = executable_name(&plist_src)?;
    let dest_bin = macos.join(&executable);
    fs::copy(bin, &dest_bin)?;
    let mut perms = fs::metadata(&dest_bin)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&dest_bin, perms)?;
    if let Some(icon) = &gpui.icon {
        let src = root.join(icon);
        if src.is_file() {
            let name = icon_dest_name(&plist_src, &src);
            fs::copy(&src, resources.join(name))?;
        } else {
            eprintln!("warning: icon {} missing", src.display());
        }
    }
    Ok(())
}

fn executable_name(plist_path: &Path) -> Result<String> {
    let value = plist::Value::from_file(plist_path)?;
    let dict = value
        .as_dictionary()
        .context("Info.plist root is not a dict")?;
    dict.get("CFBundleExecutable")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string())
        .context("Info.plist missing CFBundleExecutable")
}

fn icon_dest_name(plist_path: &Path, icon: &Path) -> String {
    let fallback = icon.file_name().unwrap().to_string_lossy().into_owned();
    let Ok(value) = plist::Value::from_file(plist_path) else {
        return fallback;
    };
    let Some(dict) = value.as_dictionary() else {
        return fallback;
    };
    match dict
        .get("CFBundleIconFile")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string())
    {
        Some(name) if name.ends_with(".icns") => name,
        Some(name) => format!("{name}.icns"),
        None => fallback,
    }
}

fn entitlements_path(root: &Path, configured: Option<&str>) -> Result<PathBuf> {
    if let Some(rel) = configured {
        let path = root.join(rel);
        if path.is_file() {
            return Ok(path);
        }
        bail!("entitlements file not found: {}", path.display());
    }
    let path = root.join("packaging/developer-id.entitlements");
    if path.is_file() {
        return Ok(path);
    }
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, macos::default_entitlements())?;
    Ok(path)
}

fn ship_tauri(root: &Path, config: &Config) -> Result<PathBuf> {
    let _ = config.tauri()?;
    let status = tauri_build(root)?.status().context("tauri build")?;
    if !status.success() {
        bail!("tauri build --bundles dmg failed");
    }
    macos::newest_with_extension(&root.join("src-tauri"), "dmg")
        .or_else(|_| macos::newest_with_extension(&root.join("target"), "dmg"))
}

fn tauri_build(root: &Path) -> Result<Command> {
    let mut cmd;
    if root.join("pnpm-lock.yaml").is_file() {
        cmd = Command::new("pnpm");
        cmd.args(["tauri", "build", "--bundles", "dmg"]);
    } else if root.join("yarn.lock").is_file() {
        cmd = Command::new("yarn");
        cmd.args(["tauri", "build", "--bundles", "dmg"]);
    } else if root.join("package-lock.json").is_file() {
        cmd = Command::new("npm");
        cmd.args(["run", "tauri", "--", "build", "--bundles", "dmg"]);
    } else {
        cmd = Command::new("cargo");
        cmd.args(["tauri", "build", "--bundles", "dmg"]);
    }
    cmd.current_dir(root);
    Ok(cmd)
}

fn ship_native(root: &Path, config: &Config) -> Result<PathBuf> {
    let native = config.native()?;
    let archive = root.join("build/apple-ship.xcarchive");
    if archive.exists() {
        fs::remove_dir_all(&archive)?;
    }
    let mut archive_cmd = Command::new("xcodebuild");
    archive_cmd
        .arg("archive")
        .arg("-scheme")
        .arg(&native.scheme)
        .arg("-configuration")
        .arg("Release")
        .arg("-archivePath")
        .arg(&archive)
        .arg("-destination")
        .arg("generic/platform=macOS")
        .arg(format!("DEVELOPMENT_TEAM={}", config.team_id));
    add_project_args(&mut archive_cmd, root, native);
    let status = archive_cmd.current_dir(root).status()?;
    if !status.success() {
        bail!("xcodebuild archive failed");
    }

    let export_dir = root.join("build/apple-ship-export");
    if export_dir.exists() {
        fs::remove_dir_all(&export_dir)?;
    }
    fs::create_dir_all(&export_dir)?;
    let plist_path = root.join("build/ExportOptions-developer-id.plist");
    fs::write(
        &plist_path,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key>
    <string>developer-id</string>
    <key>teamID</key>
    <string>{}</string>
    <key>signingStyle</key>
    <string>automatic</string>
</dict>
</plist>
"#,
            config.team_id
        ),
    )?;
    let status = Command::new("xcodebuild")
        .args([
            "-exportArchive",
            "-archivePath",
            &archive.to_string_lossy(),
            "-exportPath",
            &export_dir.to_string_lossy(),
            "-exportOptionsPlist",
            &plist_path.to_string_lossy(),
        ])
        .current_dir(root)
        .status()?;
    if !status.success() {
        bail!("xcodebuild -exportArchive failed");
    }

    let app = find_app(&export_dir)?;
    let out_dir = root.join("target/apple-ship");
    fs::create_dir_all(&out_dir)?;
    let dmg = out_dir.join(format!("{}.dmg", config.product_name));
    create_dmg(&app, &dmg, &config.product_name)?;
    let identity = find_identity(Channel::DeveloperId, &config.team_id)?;
    codesign_path(&identity, &dmg, None, false)?;
    notarize(&dmg, &config.team_id)?;
    staple(&dmg)?;
    Ok(dmg)
}

fn add_project_args(cmd: &mut Command, root: &Path, native: &NativeConfig) {
    if let Some(ws) = &native.workspace {
        cmd.arg("-workspace").arg(root.join(ws));
    } else if let Some(proj) = &native.project {
        cmd.arg("-project").arg(root.join(proj));
    }
}

fn find_app(dir: &Path) -> Result<PathBuf> {
    let mut apps = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("app") {
                apps.push(path);
            }
        }
    }
    apps.into_iter()
        .next()
        .with_context(|| format!("export produced no .app in {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GpuiConfig;
    use crate::policy::Kind;
    use tempfile::tempdir;

    #[test]
    fn assembles_gpui_bundle_layout() {
        let dir = tempdir().unwrap();
        let plist = dir.path().join("Info.plist");
        fs::write(
            &plist,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>demo</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>CFBundleIdentifier</key><string>com.demo.app</string>
</dict></plist>"#,
        )
        .unwrap();
        let bin = dir.path().join("demo");
        fs::write(&bin, b"fake").unwrap();
        let cfg = Config {
            kind: Kind::Gpui,
            team_id: "N59353RP3W".into(),
            bundle_id: "com.demo.app".into(),
            product_name: "demo".into(),
            channels: Vec::new(),
            gpui: Some(GpuiConfig {
                bin: "demo".into(),
                package: None,
                info_plist: "Info.plist".into(),
                icon: None,
                entitlements: None,
            }),
            tauri: None,
            native: None,
        };
        let app = dir.path().join("demo.app");
        assemble_gpui_app(dir.path(), &cfg, &bin, &app).unwrap();
        assert!(app.join("Contents/MacOS/demo").is_file());
        assert!(app.join("Contents/Info.plist").is_file());
    }
}
