use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{RngCore, rngs::OsRng};

const PREFIX: &str = "mctier-invite-v3:";
const AAD: &[u8] = b"MCTier/invite/v3";

pub fn export_password(password: &str) -> eyre::Result<String> {
    if password.is_empty() {
        return Ok(String::new());
    }

    let mut key = [0u8; 32];
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut key);
    OsRng.fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| {
        eyre::eyre!("could not initialize invitation encryption")
    })?;
    let encrypted = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: password.as_bytes(),
                aad: AAD,
            },
        )
        .map_err(|_| eyre::eyre!("could not encrypt the room password"))?;

    let mut envelope =
        Vec::with_capacity(key.len() + nonce.len() + encrypted.len());
    envelope.extend_from_slice(&key);
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&encrypted);
    Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(envelope)))
}

pub fn resolve_password(value: &str) -> eyre::Result<String> {
    let Some(encoded) = value.strip_prefix(PREFIX) else {
        return Ok(value.to_string());
    };
    if encoded.len() > 4096 {
        eyre::bail!("invitation secret exceeds the supported size");
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| eyre::eyre!("invalid MCTier invitation secret"))?;
    if envelope.len() < 60 {
        eyre::bail!("invalid MCTier invitation secret");
    }

    let (key, sealed) = envelope.split_at(32);
    let (nonce, ciphertext) = sealed.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| eyre::eyre!("invalid MCTier invitation key"))?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: AAD,
            },
        )
        .map_err(|_| {
            eyre::eyre!("MCTier invitation secret was modified or is invalid")
        })?;
    String::from_utf8(plaintext).map_err(|_| {
        eyre::eyre!("MCTier invitation password is not valid text")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_secret_matches_the_mctier_v3_envelope_and_round_trips() {
        let first = export_password("valid-password").unwrap();
        let second = export_password("valid-password").unwrap();
        assert!(first.starts_with(PREFIX));
        assert_ne!(first, second);
        assert_eq!(resolve_password(&first).unwrap(), "valid-password");
        assert!(!first.contains("valid-password"));
    }

    #[test]
    fn invite_secret_rejects_tampering_and_preserves_empty_or_plain_passwords()
    {
        assert_eq!(export_password("").unwrap(), "");
        assert_eq!(resolve_password("").unwrap(), "");
        assert_eq!(
            resolve_password("plain-password").unwrap(),
            "plain-password"
        );

        let mut invite = export_password("valid-password").unwrap();
        let last = invite.pop().unwrap();
        invite.push(if last == 'A' { 'B' } else { 'A' });
        assert!(resolve_password(&invite).is_err());
    }
}
