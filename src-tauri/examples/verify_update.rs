//! Verify release artifact signatures against the public key embedded in this build.
//! No installer is executed. Used by the release pipeline before publishing latest.json.
use base64::{engine::general_purpose::STANDARD, Engine};
use minisign_verify::{PublicKey, Signature};
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let key = option_env!("STARNEXUS_UPDATER_PUBLIC_KEY").ok_or("Missing StarNexus updater public key")?;
    let key = PublicKey::decode(&String::from_utf8(STANDARD.decode(key)?)?)?;
    let files: Vec<_> = std::env::args().skip(1).collect();
    if files.is_empty() {
        return Err("Specify update artifacts to verify".into());
    }
    for file in files {
        let encoded = fs::read_to_string(format!("{file}.sig"))?;
        let signature = Signature::decode(&String::from_utf8(STANDARD.decode(encoded.trim())?)?)?;
        key.verify(&fs::read(&file)?, &signature, false)?;
        println!("Verified update signature: {file}");
    }
    Ok(())
}
