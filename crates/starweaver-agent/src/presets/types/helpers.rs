#[allow(clippy::trivially_copy_pass_by_ref)]
pub(super) const fn is_false(value: &bool) -> bool {
    !*value
}

pub(super) const fn default_true() -> bool {
    true
}

pub(super) fn default_skills_dir() -> String {
    "skills".to_string()
}
