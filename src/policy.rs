use crate::cert::CertRole;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Channel {
    #[value(name = "developer-id")]
    DeveloperId,
    #[value(name = "app-store")]
    AppStore,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeveloperId => "developer-id",
            Self::AppStore => "app-store",
        }
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Gpui,
    Tauri,
    Native,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gpui => "gpui",
            Self::Tauri => "tauri",
            Self::Native => "native",
        }
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// GitHub secret that holds the p12, and the secret that holds its password.
pub fn secrets_for_role(role: CertRole) -> Option<(&'static str, &'static str)> {
    match role {
        CertRole::DeveloperIdApplication => {
            Some(("APPLE_CERTIFICATE", "APPLE_CERTIFICATE_PASSWORD"))
        }
        CertRole::AppleDistribution => Some((
            "APPLE_CERTIFICATE_APP_STORE",
            "APPLE_CERTIFICATE_APP_STORE_PASSWORD",
        )),
        CertRole::DeveloperIdInstaller => Some((
            "APPLE_CERTIFICATE_INSTALLER",
            "APPLE_CERTIFICATE_INSTALLER_PASSWORD",
        )),
        CertRole::MacInstallerDistribution => Some((
            "APPLE_CERTIFICATE_MAC_INSTALLER",
            "APPLE_CERTIFICATE_MAC_INSTALLER_PASSWORD",
        )),
        CertRole::AppleDevelopment | CertRole::Unknown => None,
    }
}

pub fn required_signing_secrets(channel: Channel) -> &'static [&'static str] {
    match channel {
        Channel::DeveloperId => &[
            "APPLE_CERTIFICATE",
            "APPLE_CERTIFICATE_PASSWORD",
            "APPLE_TEAM_ID",
        ],
        Channel::AppStore => &[
            "APPLE_CERTIFICATE_APP_STORE",
            "APPLE_CERTIFICATE_APP_STORE_PASSWORD",
            "APPLE_TEAM_ID",
        ],
    }
}

/// Either Apple ID + app-specific password, or an App Store Connect API key.
pub fn notary_secret_groups() -> &'static [&'static [&'static str]] {
    &[
        &["APPLE_ID", "APPLE_APP_SPECIFIC_PASSWORD"],
        &["APPLE_API_KEY", "APPLE_API_ISSUER", "APPLE_API_KEY_P8"],
    ]
}

pub fn ci_supports(channel: Channel) -> bool {
    matches!(channel, Channel::DeveloperId)
}

pub fn application_role(channel: Channel) -> CertRole {
    match channel {
        Channel::DeveloperId => CertRole::DeveloperIdApplication,
        Channel::AppStore => CertRole::AppleDistribution,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developer_id_secrets() {
        assert!(required_signing_secrets(Channel::DeveloperId).contains(&"APPLE_CERTIFICATE"));
        assert_eq!(
            secrets_for_role(CertRole::DeveloperIdApplication),
            Some(("APPLE_CERTIFICATE", "APPLE_CERTIFICATE_PASSWORD"))
        );
    }

    #[test]
    fn app_store_uses_distribution_cert() {
        assert_eq!(
            application_role(Channel::AppStore),
            CertRole::AppleDistribution
        );
        assert!(
            required_signing_secrets(Channel::AppStore).contains(&"APPLE_CERTIFICATE_APP_STORE")
        );
    }

    #[test]
    fn development_certs_have_no_ship_secrets() {
        assert_eq!(secrets_for_role(CertRole::AppleDevelopment), None);
    }

    #[test]
    fn ci_does_not_ship_app_store_yet() {
        assert!(ci_supports(Channel::DeveloperId));
        assert!(!ci_supports(Channel::AppStore));
    }
}
