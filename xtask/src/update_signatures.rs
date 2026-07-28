use std::{env, fs, path::Path};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use minisign_verify::{PublicKey, Signature};

const MAX_ARTIFACT_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 16 * 1024;

pub fn verify(args: &[String]) -> Result<(), String> {
    if args.len() != 2 {
        return Err(
            "usage: cargo run -p xtask -- verify-update-signature <artifact> <signature>"
                .to_string(),
        );
    }
    let configured = env::var("STARWEAVER_UPDATE_PUBLIC_KEY")
        .map_err(|_| "STARWEAVER_UPDATE_PUBLIC_KEY is required".to_string())?;
    let public_key = parse_public_key(configured.trim())?;
    let artifact_path = Path::new(&args[0]);
    let signature_path = Path::new(&args[1]);
    let artifact = read_bounded(artifact_path, MAX_ARTIFACT_BYTES, "artifact")?;
    let signature_bytes = read_bounded(signature_path, MAX_SIGNATURE_BYTES, "signature")?;
    let signature_text = std::str::from_utf8(&signature_bytes)
        .map_err(|_| "update signature is not UTF-8".to_string())?;
    let signature = parse_signature(signature_text)?;
    public_key
        .verify(&artifact, &signature, false)
        .map_err(|_| "update signature does not match STARWEAVER_UPDATE_PUBLIC_KEY".to_string())?;
    println!("verified update signature for {}", artifact_path.display());
    Ok(())
}

fn read_bounded(path: &Path, maximum: u64, label: &str) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path).map_err(|_| format!("{label} is unavailable"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum {
        return Err(format!("{label} size is invalid"));
    }
    fs::read(path).map_err(|_| format!("{label} could not be read"))
}

fn parse_public_key(configured: &str) -> Result<PublicKey, String> {
    if configured.starts_with("untrusted comment:") {
        return PublicKey::decode(configured)
            .map_err(|_| "update public key is invalid".to_string());
    }
    if let Ok(decoded) = BASE64.decode(configured)
        && let Ok(text) = std::str::from_utf8(&decoded)
    {
        return PublicKey::decode(text).map_err(|_| "update public key is invalid".to_string());
    }
    PublicKey::from_base64(configured).map_err(|_| "update public key is invalid".to_string())
}

fn parse_signature(signature: &str) -> Result<Signature, String> {
    let signature = signature.trim();
    if signature.starts_with("untrusted comment:") {
        return Signature::decode(signature).map_err(|_| "update signature is invalid".to_string());
    }
    let decoded = BASE64
        .decode(signature)
        .map_err(|_| "update signature is invalid".to_string())?;
    let text =
        std::str::from_utf8(&decoded).map_err(|_| "update signature is invalid".to_string())?;
    Signature::decode(text).map_err(|_| "update signature is invalid".to_string())
}
