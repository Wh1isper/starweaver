//! Build-time target identity for the standalone RPC product.

fn main() {
    let Ok(target) = std::env::var("TARGET") else {
        panic!("Cargo must provide the RPC target triple");
    };
    println!("cargo:rustc-env=STARWEAVER_TARGET_TRIPLE={target}");
    println!("cargo:rerun-if-env-changed=STARWEAVER_BUILD_REVISION");
}
