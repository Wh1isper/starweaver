//! Build-time target identity for the standalone RPC product.

fn main() {
    let Ok(target) = std::env::var("TARGET") else {
        panic!("Cargo must provide the RPC target triple");
    };
    let version = std::env::var("STARWEAVER_RELEASE_VERSION")
        .or_else(|_| std::env::var("CARGO_PKG_VERSION"))
        .unwrap_or_else(|_| "0.0.0-dev.0".to_owned());
    let revision =
        std::env::var("STARWEAVER_BUILD_REVISION").unwrap_or_else(|_| "development".to_owned());
    println!("cargo:rustc-env=STARWEAVER_TARGET_TRIPLE={target}");
    println!("cargo:rustc-env=STARWEAVER_BUILD_VERSION={version}");
    println!("cargo:rustc-env=STARWEAVER_BUILD_REVISION={revision}");
    println!("cargo:rerun-if-env-changed=STARWEAVER_RELEASE_VERSION");
    println!("cargo:rerun-if-env-changed=STARWEAVER_BUILD_REVISION");
}
