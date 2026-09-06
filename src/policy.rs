use crate::cert::CertRole;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, clap::ValueEnum, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
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

pub fn ci_supports(channel: Channel) -> bool {
    matches!(channel, Channel::DeveloperId)
}

pub fn application_role(channel: Channel) -> CertRole {
    match channel {
        Channel::DeveloperId => CertRole::DeveloperIdApplication,
        Channel::AppStore => CertRole::AppleDistribution,
    }
}

/// The single signing cert in `infos` that this channel requires.
pub fn cert_matching_channel(
    channel: Channel,
    infos: &[crate::cert::CertInfo],
) -> Option<&crate::cert::CertInfo> {
    let want = application_role(channel);
    let mut matched = infos.iter().filter(|info| info.role == want);
    match (matched.next(), matched.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developer_id_secrets() {
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
        assert_eq!(
            secrets_for_role(CertRole::AppleDistribution),
            Some((
                "APPLE_CERTIFICATE_APP_STORE",
                "APPLE_CERTIFICATE_APP_STORE_PASSWORD"
            ))
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

    #[test]
    fn matching_cert_is_channel_specific() {
        use crate::cert::{CertInfo, CertRole};
        let did = CertInfo {
            role: CertRole::DeveloperIdApplication,
            team_id: "N59353RP3W".into(),
            identity: "Developer ID Application: wanqiu gao (N59353RP3W)".into(),
            common_name: "Developer ID Application: wanqiu gao (N59353RP3W)".into(),
        };
        let dist = CertInfo {
            role: CertRole::AppleDistribution,
            team_id: "N59353RP3W".into(),
            identity: "Apple Distribution: wanqiu gao (N59353RP3W)".into(),
            common_name: "Apple Distribution: wanqiu gao (N59353RP3W)".into(),
        };
        let both = [did.clone(), dist.clone()];
        assert_eq!(
            cert_matching_channel(Channel::DeveloperId, &both)
                .unwrap()
                .role,
            CertRole::DeveloperIdApplication
        );
        assert_eq!(
            cert_matching_channel(Channel::AppStore, &both)
                .unwrap()
                .role,
            CertRole::AppleDistribution
        );
        assert!(cert_matching_channel(Channel::AppStore, &[did]).is_none());
        assert!(cert_matching_channel(Channel::DeveloperId, &[dist]).is_none());
    }
}
