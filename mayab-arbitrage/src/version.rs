use serde::Serialize;

/// Identidad inmutable de la revision compilada. Los valores se inyectan al
/// construir la imagen; los fallbacks mantienen builds locales reproducibles.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildVersion {
    pub git_sha: &'static str,
    pub build_time: &'static str,
    pub version: &'static str,
    pub environment: &'static str,
}

pub const fn current() -> BuildVersion {
    BuildVersion {
        git_sha: match option_env!("MAYAB_GIT_SHA") {
            Some(value) => value,
            None => "local",
        },
        build_time: match option_env!("MAYAB_BUILD_TIME") {
            Some(value) => value,
            None => "not-recorded",
        },
        version: match option_env!("MAYAB_RELEASE_VERSION") {
            Some(value) => value,
            None => env!("CARGO_PKG_VERSION"),
        },
        environment: match option_env!("MAYAB_BUILD_ENV") {
            Some(value) => value,
            None => "development",
        },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_has_non_empty_identity_fields() {
        let version = super::current();
        assert!(!version.git_sha.is_empty());
        assert!(!version.build_time.is_empty());
        assert!(!version.version.is_empty());
        assert!(!version.environment.is_empty());
    }
}
