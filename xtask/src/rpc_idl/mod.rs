mod bundle;
mod check;
mod fixtures;
mod generate_desktop;
mod generate_rust;
mod generate_typescript;
mod lint;
mod model;
mod source;
mod validate;

#[cfg(test)]
mod tests;

pub use check::{check_all, check_source, generate};
pub use generate_typescript::generate_to as generate_typescript;

pub fn check_fixtures(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("check-rpc-idl-fixtures takes no arguments".to_string());
    }
    fixtures::check(&crate::common::root()?)
}
