//! Build-time target identity for release diagnostics.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown-target".to_owned());
    println!("cargo:rustc-env=STARWEAVER_BUILD_TARGET={target}");
}
