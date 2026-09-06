use anyhow::{bail, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl SemVer {
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.strip_prefix('v').unwrap_or(s).trim();
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            bail!("version must be X.Y.Z, got {s}");
        }
        let parse = |p: &str, name: &str| {
            p.parse::<u64>()
                .map_err(|_| anyhow::anyhow!("invalid {name} in version {s}"))
        };
        Ok(Self {
            major: parse(parts[0], "major")?,
            minor: parse(parts[1], "minor")?,
            patch: parse(parts[2], "patch")?,
        })
    }

    pub fn bump(self, spec: ReleaseSpec) -> Result<Self> {
        match spec {
            ReleaseSpec::Major => Ok(Self {
                major: self.major + 1,
                minor: 0,
                patch: 0,
            }),
            ReleaseSpec::Minor => Ok(Self {
                major: self.major,
                minor: self.minor + 1,
                patch: 0,
            }),
            ReleaseSpec::Patch => Ok(Self {
                major: self.major,
                minor: self.minor,
                patch: self.patch + 1,
            }),
            ReleaseSpec::Exact(v) => {
                if v <= self {
                    bail!("new version {} is not greater than current {}", v, self);
                }
                Ok(v)
            }
        }
    }
}

impl std::fmt::Display for SemVer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseSpec {
    Major,
    Minor,
    Patch,
    Exact(SemVer),
}

pub fn parse_spec(s: &str) -> Result<ReleaseSpec> {
    match s.trim() {
        "major" => Ok(ReleaseSpec::Major),
        "minor" => Ok(ReleaseSpec::Minor),
        "patch" => Ok(ReleaseSpec::Patch),
        "path" => bail!("unknown spec `path`; use patch, minor, major, or vX.Y.Z"),
        other => Ok(ReleaseSpec::Exact(SemVer::parse(other)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_v_prefix() {
        assert_eq!(
            SemVer::parse("v1.2.3").unwrap(),
            SemVer {
                major: 1,
                minor: 2,
                patch: 3
            }
        );
        assert_eq!(SemVer::parse("1.2.3").unwrap().to_string(), "1.2.3");
    }

    #[test]
    fn bump_parts() {
        let v = SemVer::parse("1.2.3").unwrap();
        assert_eq!(v.bump(ReleaseSpec::Major).unwrap().to_string(), "2.0.0");
        assert_eq!(v.bump(ReleaseSpec::Minor).unwrap().to_string(), "1.3.0");
        assert_eq!(v.bump(ReleaseSpec::Patch).unwrap().to_string(), "1.2.4");
    }

    #[test]
    fn exact_must_increase() {
        let v = SemVer::parse("1.0.1").unwrap();
        let spec = parse_spec("v1.0.2").unwrap();
        assert_eq!(v.bump(spec).unwrap().to_string(), "1.0.2");
        assert!(v.bump(parse_spec("1.0.1").unwrap()).is_err());
        assert!(v.bump(parse_spec("1.0.0").unwrap()).is_err());
    }

    #[test]
    fn path_is_rejected() {
        let err = parse_spec("path").unwrap_err().to_string();
        assert!(err.contains("patch"));
    }
}
