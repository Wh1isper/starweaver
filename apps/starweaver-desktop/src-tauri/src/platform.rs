use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopPlatform {
    Linux,
    Macos,
    Windows,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub platform: DesktopPlatform,
    pub architecture: String,
}

impl PlatformInfo {
    pub fn current() -> Self {
        let platform = match std::env::consts::OS {
            "linux" => DesktopPlatform::Linux,
            "macos" => DesktopPlatform::Macos,
            "windows" => DesktopPlatform::Windows,
            _ => DesktopPlatform::Unknown,
        };
        Self {
            platform,
            architecture: std::env::consts::ARCH.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_platform_has_a_nonempty_architecture() {
        let platform = PlatformInfo::current();

        assert!(!platform.architecture.is_empty());
        assert_ne!(platform.platform, DesktopPlatform::Unknown);
    }
}
