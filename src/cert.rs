use std::path::Path;
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

pub fn inspect_x509(path: &Path) -> Result<CertInfo> {
    let subject = openssl_subject_from_file(path)?;
    let info = classify_subject(&subject)?;
    if is_intermediate_cn(&info.common_name) {
        bail!(
            "{} looks like an Apple intermediate, not a signing certificate",
            path.display()
        );
    }
    Ok(info)
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
}
