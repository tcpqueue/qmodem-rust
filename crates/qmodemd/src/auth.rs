use crate::config;
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::path::Path;
use subtle::ConstantTimeEq;

pub fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
pub fn authorized(stored: &str, supplied: &str) -> bool {
    !stored.is_empty()
        && supplied.len() <= 256
        && bool::from(stored.as_bytes().ct_eq(digest(supplied).as_bytes()))
}

/// Printed only to the authenticated CLI caller, never to logs or service-info.
pub fn initialize(path: &Path) -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("generate token: {e}"))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    config::update(path, |d| {
        ensure!(
            d.get("auth")
                .and_then(|a| a.get("token_hash"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .is_empty(),
            "an access token is already configured; initialization will not overwrite it"
        );
        d["auth"]["token_hash"] = toml_edit::value(digest(&token));
        Ok(())
    })?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_check_never_accepts_empty_or_wrong_tokens() {
        let hash = digest("example-secret");
        assert!(authorized(&hash, "example-secret"));
        assert!(!authorized(&hash, "wrong"));
        assert!(!authorized("", ""));
    }
    #[test]
    fn initialization_does_not_reset_existing_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("q.toml");
        std::fs::write(&path, include_str!("../../../config/qmodem.example.toml")).unwrap();
        let token = initialize(&path).unwrap();
        let cfg = config::Config::load(&path).unwrap();
        assert!(authorized(&cfg.auth.token_hash, &token));
        assert!(!std::fs::read_to_string(&path).unwrap().contains(&token));
        assert!(initialize(&path).is_err());
    }
}
