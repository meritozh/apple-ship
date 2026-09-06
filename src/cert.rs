use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CertRole {
    DeveloperIdApplication,
    AppleDistribution,
    DeveloperIdInstaller,
    MacInstallerDistribution,
    AppleDevelopment,
    Unknown,
}

impl CertRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeveloperIdApplication => "Developer ID Application",
            Self::AppleDistribution => "Apple Distribution",
            Self::DeveloperIdInstaller => "Developer ID Installer",
            Self::MacInstallerDistribution => "Mac Installer Distribution",
            Self::AppleDevelopment => "Apple Development",
            Self::Unknown => "unknown",
        }
    }

    pub fn is_release(self) -> bool {
        matches!(
            self,
            Self::DeveloperIdApplication
                | Self::AppleDistribution
                | Self::DeveloperIdInstaller
                | Self::MacInstallerDistribution
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertInfo {
    pub role: CertRole,
    pub team_id: String,
    pub identity: String,
    pub common_name: String,
}

#[derive(Debug, Error)]
pub enum CertError {
    #[error("certificate subject has no recognizable CN: {0}")]
    NoCommonName(String),
    #[error("could not find a 10-character Team ID in certificate subject: {0}")]
    NoTeamId(String),
}

pub fn classify_subject(subject: &str) -> Result<CertInfo, CertError> {
    let subject = subject.trim();
    let subject = subject.strip_prefix("subject=").unwrap_or(subject).trim();
    let cn = extract_cn(subject).ok_or_else(|| CertError::NoCommonName(subject.to_string()))?;
    let role = role_from_cn(&cn);
    let team_id = extract_team_id(subject, &cn).unwrap_or_default();
    if team_id.is_empty() && role.is_release() {
        return Err(CertError::NoTeamId(subject.to_string()));
    }
    Ok(CertInfo {
        role,
        identity: cn.clone(),
        common_name: cn,
        team_id,
    })
}

fn role_from_cn(cn: &str) -> CertRole {
    let cn = cn.trim();
    if cn.starts_with("Developer ID Application:") {
        CertRole::DeveloperIdApplication
    } else if cn.starts_with("Apple Distribution:")
        || cn.starts_with("3rd Party Mac Developer Application:")
    {
        CertRole::AppleDistribution
    } else if cn.starts_with("Developer ID Installer:") {
        CertRole::DeveloperIdInstaller
    } else if cn.starts_with("3rd Party Mac Developer Installer:")
        || cn.starts_with("Mac Installer Distribution:")
    {
        CertRole::MacInstallerDistribution
    } else if cn.starts_with("Apple Development:") || cn.starts_with("Mac Developer:") {
        CertRole::AppleDevelopment
    } else {
        CertRole::Unknown
    }
}

fn extract_cn(subject: &str) -> Option<String> {
    if let Some(rest) = subject.strip_prefix('/') {
        for part in rest.split('/') {
            if let Some(value) = part.strip_prefix("CN=") {
                return Some(value.to_string());
            }
        }
    }
    for part in split_dn(subject) {
        if let Some(value) = part.strip_prefix("CN=") {
            return Some(value.to_string());
        }
    }
    None
}

fn split_dn(subject: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let chars: Vec<char> = subject.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] == ',' {
            let byte_start = subject
                .char_indices()
                .nth(start)
                .map(|(b, _)| b)
                .unwrap_or(0);
            let byte_end = subject
                .char_indices()
                .nth(i)
                .map(|(b, _)| b)
                .unwrap_or(subject.len());
            parts.push(subject[byte_start..byte_end].trim());
            start = i + 1;
        }
        i += 1;
    }
    if start < chars.len() {
        let byte_start = subject
            .char_indices()
            .nth(start)
            .map(|(b, _)| b)
            .unwrap_or(0);
        parts.push(subject[byte_start..].trim());
    }
    parts
}

fn extract_team_id(subject: &str, cn: &str) -> Option<String> {
    for part in split_dn(subject)
        .into_iter()
        .chain(subject.trim_start_matches('/').split('/').map(|s| s.trim()))
    {
        if let Some(ou) = part.strip_prefix("OU=") {
            if is_team_id(ou) {
                return Some(ou.to_string());
            }
        }
    }
    team_id_from_parens(cn)
}

fn team_id_from_parens(cn: &str) -> Option<String> {
    let start = cn.rfind('(')?;
    let end = cn.rfind(')')?;
    if end <= start + 1 {
        return None;
    }
    let inner = &cn[start + 1..end];
    is_team_id(inner).then(|| inner.to_string())
}

fn is_team_id(s: &str) -> bool {
    s.len() == 10 && s.chars().all(|c| c.is_ascii_alphanumeric())
}

pub fn is_intermediate_cn(cn: &str) -> bool {
    let cn = cn.to_ascii_lowercase();
    cn.contains("worldwide developer relations")
        || cn.contains("apple root")
        || cn.contains("developer authentication")
        || cn.contains("apple intermediate")
}

pub fn inspect_p12(path: &Path, password: &str) -> Result<Vec<CertInfo>> {
    let pem = openssl_pkcs12_certs(path, password)?;
    let mut infos = Vec::new();
    for der_or_pem in split_pem_certs(&pem) {
        let subject = openssl_subject_from_pem(&der_or_pem)?;
        match classify_subject(&subject) {
            Ok(info) if is_intermediate_cn(&info.common_name) || info.role == CertRole::Unknown => {
                continue;
            }
            Ok(info) => infos.push(info),
            Err(CertError::NoCommonName(_)) => continue,
            Err(err) => return Err(err.into()),
        }
    }
    if infos.is_empty() {
        bail!(
            "{} contains no Developer ID, Apple Distribution, or Installer certificate",
            path.display()
        );
    }
    Ok(infos)
}

fn openssl_subject_from_file(path: &Path) -> Result<String> {
    for inform in ["PEM", "DER"] {
        let output = Command::new("openssl")
            .args([
                "x509",
                "-noout",
                "-subject",
                "-nameopt",
                "RFC2253",
                "-inform",
                inform,
                "-in",
                &path.to_string_lossy(),
            ])
            .output()
            .context("failed to run openssl x509")?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
        }
    }
    bail!("could not parse {} as PEM or DER X.509", path.display());
}

/// If `path` is a .p12, return it. If it is a directory, pick the unique .p12
/// whose sibling .cer/.crt/.pem matches `want`. Does not open PKCS#12 files
/// (those need a password).
pub fn resolve_p12(path: &Path, want: CertRole) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("certificate path not found: {}", path.display()))?;
    if path.is_file() {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "p12" || ext == "pfx" {
            return Ok(path);
        }
        bail!(
            "{} is not a PKCS#12 file. Pass a .p12 or a folder of certificates.",
            path.display()
        );
    }
    if !path.is_dir() {
        bail!("{} is not a file or directory", path.display());
    }

    let mut matches: Vec<PathBuf> = Vec::new();
    let mut inspected = Vec::new();
    for entry in fs::read_dir(&path).with_context(|| format!("read {}", path.display()))? {
        let entry = entry?;
        let file = entry.path();
        if !file.is_file() {
            continue;
        }
        let ext = file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "cer" | "crt" | "der" | "pem") {
            continue;
        }
        inspected.push(file.file_name().unwrap().to_string_lossy().into_owned());
        let subject = openssl_subject_from_file(&file)?;
        let info = match classify_subject(&subject) {
            Ok(info) => info,
            Err(_) => continue,
        };
        if is_intermediate_cn(&info.common_name) || info.role != want {
            continue;
        }
        let stem = file.file_stem().context("certificate file name")?;
        let p12 = ["p12", "pfx"]
            .iter()
            .map(|ext| {
                let mut p = file.with_file_name(stem);
                p.set_extension(ext);
                p
            })
            .find(|p| p.is_file());
        let Some(p12) = p12 else {
            bail!(
                "found {} in {} but there is no sibling .p12 with the private key",
                file.file_name().unwrap_or_default().to_string_lossy(),
                path.display()
            );
        };
        matches.push(p12);
    }

    match matches.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!(
            "no {} certificate in {}. Inspected: {}",
            want.as_str(),
            path.display(),
            if inspected.is_empty() {
                "(no .cer/.crt/.pem files)".to_string()
            } else {
                inspected.join(", ")
            }
        ),
        many => bail!(
            "multiple {} certificates in {}: {}",
            want.as_str(),
            path.display(),
            many.iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn openssl_pkcs12_certs(path: &Path, password: &str) -> Result<String> {
    let output = Command::new("openssl")
        .args([
            "pkcs12",
            "-in",
            &path.to_string_lossy(),
            "-nokeys",
            "-clcerts",
            "-passin",
            "env:APPLE_SHIP_P12_PASS",
        ])
        .env("APPLE_SHIP_P12_PASS", password)
        .output()
        .context("failed to run openssl pkcs12")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("openssl pkcs12 could not read {}: {stderr}", path.display());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn split_pem_certs(pem: &str) -> Vec<String> {
    let mut certs = Vec::new();
    let mut current = String::new();
    let mut in_cert = false;
    for line in pem.lines() {
        if line.contains("BEGIN CERTIFICATE") {
            in_cert = true;
            current.clear();
            current.push_str(line);
            current.push('\n');
            continue;
        }
        if in_cert {
            current.push_str(line);
            current.push('\n');
            if line.contains("END CERTIFICATE") {
                certs.push(std::mem::take(&mut current));
                in_cert = false;
            }
        }
    }
    certs
}

fn openssl_subject_from_pem(pem: &str) -> Result<String> {
    let mut child = Command::new("openssl")
        .args(["x509", "-noout", "-subject", "-nameopt", "RFC2253"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn openssl x509")?;
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .context("openssl stdin")?
            .write_all(pem.as_bytes())?;
    }
    let output = child.wait_with_output().context("openssl x509")?;
    if !output.status.success() {
        bail!(
            "openssl x509 failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_developer_id_rfc2253() {
        let s =
            "CN=Developer ID Application: wanqiu gao (N59353RP3W),OU=N59353RP3W,O=wanqiu gao,C=US";
        let info = classify_subject(s).unwrap();
        assert_eq!(info.role, CertRole::DeveloperIdApplication);
        assert_eq!(info.team_id, "N59353RP3W");
        assert_eq!(
            info.identity,
            "Developer ID Application: wanqiu gao (N59353RP3W)"
        );
    }

    #[test]
    fn classify_distribution_slash_form() {
        let s = "subject=/CN=Apple Distribution: wanqiu gao (N59353RP3W)/OU=N59353RP3W/O=wanqiu gao/C=US";
        let info = classify_subject(s).unwrap();
        assert_eq!(info.role, CertRole::AppleDistribution);
        assert_eq!(info.team_id, "N59353RP3W");
    }

    #[test]
    fn classify_development() {
        let s = "CN=Apple Development: ah841814092@gmail.com (ZJUFZ2R7YR),OU=ZJUFZ2R7YR,O=wanqiu gao,C=US";
        let info = classify_subject(s).unwrap();
        assert_eq!(info.role, CertRole::AppleDevelopment);
        assert!(!info.role.is_release());
    }

    #[test]
    fn classify_installer() {
        let s =
            "CN=Developer ID Installer: wanqiu gao (N59353RP3W),OU=N59353RP3W,O=wanqiu gao,C=US";
        let info = classify_subject(s).unwrap();
        assert_eq!(info.role, CertRole::DeveloperIdInstaller);
    }

    #[test]
    fn classify_mac_installer_distribution() {
        let s = "CN=3rd Party Mac Developer Installer: wanqiu gao (N59353RP3W),OU=N59353RP3W,O=wanqiu gao,C=US";
        let info = classify_subject(s).unwrap();
        assert_eq!(info.role, CertRole::MacInstallerDistribution);
    }

    #[test]
    fn unknown_cn_is_unknown() {
        let s = "CN=Apple Worldwide Developer Relations Certification Authority,OU=G3,O=Apple Inc.,C=US";
        let info = classify_subject(s).unwrap();
        assert_eq!(info.role, CertRole::Unknown);
        assert!(is_intermediate_cn(&info.common_name));
    }

    fn write_cer(dir: &std::path::Path, stem: &str, cn: &str) {
        let key = dir.join(format!("{stem}.key"));
        let cer = dir.join(format!("{stem}.cer"));
        let status = Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                &key.to_string_lossy(),
                "-out",
                &cer.to_string_lossy(),
                "-days",
                "1",
                "-subj",
                &format!("/CN={cn}/OU=N59353RP3W/O=Test/C=US"),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::write(dir.join(format!("{stem}.p12")), b"placeholder").unwrap();
    }

    #[test]
    fn resolve_p12_file_passthrough() {
        let dir = tempfile::tempdir().unwrap();
        let p12 = dir.path().join("one.p12");
        std::fs::write(&p12, b"placeholder").unwrap();
        let got = resolve_p12(&p12, CertRole::DeveloperIdApplication).unwrap();
        assert_eq!(got, p12.canonicalize().unwrap());
    }

    #[test]
    fn resolve_p12_picks_cer_matching_channel() {
        let dir = tempfile::tempdir().unwrap();
        write_cer(
            dir.path(),
            "developerID_application",
            "Developer ID Application: Test (N59353RP3W)",
        );
        write_cer(
            dir.path(),
            "distribution",
            "Apple Distribution: Test (N59353RP3W)",
        );
        let did = resolve_p12(dir.path(), CertRole::DeveloperIdApplication).unwrap();
        assert_eq!(
            did.file_name().unwrap().to_str().unwrap(),
            "developerID_application.p12"
        );
        let dist = resolve_p12(dir.path(), CertRole::AppleDistribution).unwrap();
        assert_eq!(
            dist.file_name().unwrap().to_str().unwrap(),
            "distribution.p12"
        );
    }

    #[test]
    fn resolve_p12_errors_without_matching_cer() {
        let dir = tempfile::tempdir().unwrap();
        write_cer(
            dir.path(),
            "distribution",
            "Apple Distribution: Test (N59353RP3W)",
        );
        let err = resolve_p12(dir.path(), CertRole::DeveloperIdApplication).unwrap_err();
        assert!(err.to_string().contains("Developer ID Application"));
    }

    #[test]
    fn resolve_p12_errors_when_cer_has_no_sibling_p12() {
        let dir = tempfile::tempdir().unwrap();
        write_cer(
            dir.path(),
            "developerID_application",
            "Developer ID Application: Test (N59353RP3W)",
        );
        std::fs::remove_file(dir.path().join("developerID_application.p12")).unwrap();
        let err = resolve_p12(dir.path(), CertRole::DeveloperIdApplication).unwrap_err();
        assert!(err.to_string().contains("sibling .p12"));
    }
}
