//! Build identity.

use marsad_proto::daemon::VersionInfo;

/// Environment variable that overrides the reported build id. For development and tests only
/// (it makes a daemon claim to be another build, e.g. to exercise re-provisioning).
pub const BUILD_OVERRIDE_ENV: &str = "MARSAD_BUILD_ID";

/// The exact build identifier: `<version>+<git commit>[.dirty]`.
pub fn build_id() -> String {
    std::env::var(BUILD_OVERRIDE_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| env!("MARSAD_BUILD").to_string())
}

/// The Rust target triple this binary was built for.
pub fn target() -> &'static str {
    env!("MARSAD_TARGET")
}

pub fn info() -> VersionInfo {
    VersionInfo {
        build: build_id(),
        target: target().to_string(),
    }
}
