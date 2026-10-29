//! Reading the bot's trading key.
//!
//! `bot.key` holds one key, in whichever form you have it:
//!
//! - **base58**, as the Bullet app and Phantom export it (the usual case),
//! - **hex** (64 characters, optional `0x`),
//! - the **JSON array** `cargo run -- keygen` writes (Solana CLI format).
//!
//! Paste it in as-is; surrounding spaces and newlines are fine. It must be
//! the *private* key: an address looks just the same (base58, 32 bytes), so
//! if you paste one by mistake you get a different key, and the bot will say
//! that key isn't delegated.

use std::path::Path;

use anyhow::{Context, Result, bail};
use bullet_rust_sdk::Keypair;

pub fn load(path: &Path) -> Result<Keypair> {
    let text = std::fs::read_to_string(path).with_context(|| {
        format!(
            "reading the key file {} (paste your delegate key into it, or run `cargo run -- keygen`)",
            path.display()
        )
    })?;
    parse(&text).with_context(|| format!("reading the key in {}", path.display()))
}

fn parse(text: &str) -> Result<Keypair> {
    let text = text.trim();
    if text.is_empty() {
        bail!("the key file is empty");
    }
    let bytes: Vec<u8> = if text.starts_with('[') {
        serde_json::from_str(text).context("looks like a JSON key but isn't valid JSON")?
    } else if let Some(hex) = as_hex(text) {
        return Keypair::from_hex(hex).map_err(|e| anyhow::anyhow!("bad hex key: {e}"));
    } else {
        bs58::decode(text)
            .into_vec()
            .context("not a base58, hex or JSON key")?
    };
    // A secret key is 32 bytes. Exports often append the 32-byte public key (64 in all).
    match bytes.len() {
        32 | 64 => {
            let mut secret = [0u8; 32];
            secret.copy_from_slice(&bytes[..32]);
            Ok(Keypair::from_bytes(secret))
        }
        n => bail!("a private key is 32 or 64 bytes, this one is {n}"),
    }
}

fn as_hex(text: &str) -> Option<&str> {
    let body = text.strip_prefix("0x").unwrap_or(text);
    (body.len() == 64 && body.bytes().all(|b| b.is_ascii_hexdigit())).then_some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: [u8; 32] = [7; 32];

    fn address() -> String {
        Keypair::from_bytes(SECRET).address()
    }

    #[test]
    fn base58_from_the_bullet_app() {
        let key = bs58::encode(SECRET).into_string();
        assert_eq!(parse(&format!("  {key}\n")).unwrap().address(), address());
    }

    #[test]
    fn base58_with_the_public_key_appended() {
        let mut full = SECRET.to_vec();
        full.extend(Keypair::from_bytes(SECRET).public_key());
        assert_eq!(
            parse(&bs58::encode(full).into_string()).unwrap().address(),
            address()
        );
    }

    #[test]
    fn hex_with_or_without_0x() {
        let hex: String = SECRET.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(parse(&hex).unwrap().address(), address());
        assert_eq!(parse(&format!("0x{hex}")).unwrap().address(), address());
    }

    #[test]
    fn the_file_keygen_writes() {
        let dir = std::env::temp_dir().join(format!("hft-key-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bot.key");
        Keypair::from_bytes(SECRET).write_to_file(&path).unwrap();
        assert_eq!(load(&path).unwrap().address(), address());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_empty_and_garbage() {
        assert!(parse("  \n").is_err());
        assert!(parse("not a key!").is_err());
    }
}
