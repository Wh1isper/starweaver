use std::{collections::BTreeSet, ffi::OsString};

#[cfg(unix)]
pub(super) fn credential_bytes(value: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;
    value.as_bytes().to_vec()
}

#[cfg(not(unix))]
pub(super) fn credential_bytes(value: &std::ffi::OsStr) -> Vec<u8> {
    value.to_string_lossy().into_owned().into_bytes()
}

pub(super) fn allowed_credential_environment(name: &str) -> bool {
    let canonical = name.to_ascii_uppercase();
    !canonical.starts_with("LD_")
        && !canonical.starts_with("DYLD_")
        && !canonical.starts_with("STARWEAVER_")
        && !matches!(canonical.as_str(), "PATH" | "COMSPEC")
}

pub(super) fn safe_environment_names(credentials: &[String]) -> Vec<OsString> {
    let mut names = BTreeSet::<OsString>::new();
    for name in [
        "HOME",
        "USERPROFILE",
        "TMPDIR",
        "TEMP",
        "TMP",
        "SYSTEMROOT",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        names.insert(OsString::from(name));
    }
    names.extend(
        credentials
            .iter()
            .filter(|name| allowed_credential_environment(name))
            .map(OsString::from),
    );
    names.into_iter().collect()
}
