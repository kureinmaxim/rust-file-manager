//! Stateless signed tokens for the Mini App.
//!
//! The Mini App cannot rely on cookies (inside web.telegram.org it runs in a
//! third-party iframe), and `<img>`, `<video>` and Telegram's native download
//! dialog cannot send an Authorization header. So the API uses:
//!
//! * access tokens — `Authorization: Bearer v1.<payload>.<mac>`, kept only in
//!   the page's memory;
//! * signed URLs — `/d/<payload>.<mac>`, short-lived and bound to one file.
//!
//! Both are HMAC-SHA256 over a JSON payload with keys derived (HKDF) from the
//! session secret, with a separate key per purpose. Revocation works through
//! the user's `token_version`, embedded in every token and re-checked on use.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::storage::Zone;

type HmacSha256 = Hmac<Sha256>;

const ACCESS_PREFIX: &str = "v1.";
/// Access tokens live as long as a web session.
pub const ACCESS_TTL_SECS: u64 = 12 * 60 * 60;

#[derive(Clone)]
pub struct TokenKeys {
    access: [u8; 32],
    url: [u8; 32],
}

impl TokenKeys {
    pub fn derive(secret: &[u8]) -> Self {
        let hk = Hkdf::<Sha256>::new(None, secret);
        let mut access = [0u8; 32];
        let mut url = [0u8; 32];
        hk.expand(b"rfm/v1/access", &mut access)
            .expect("32 bytes is a valid HKDF length");
        hk.expand(b"rfm/v1/url", &mut url)
            .expect("32 bytes is a valid HKDF length");
        Self { access, url }
    }
}

fn mac(key: &[u8; 32]) -> HmacSha256 {
    HmacSha256::new_from_slice(key).expect("HMAC accepts any key length")
}

fn sign<T: Serialize>(key: &[u8; 32], claims: &T) -> String {
    let payload = serde_json::to_vec(claims).expect("claims serialize");
    let mut m = mac(key);
    m.update(&payload);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&payload),
        URL_SAFE_NO_PAD.encode(m.finalize().into_bytes())
    )
}

fn verify<T: DeserializeOwned>(key: &[u8; 32], token: &str) -> Option<T> {
    let (payload_b64, mac_b64) = token.split_once('.')?;
    let payload = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let tag = URL_SAFE_NO_PAD.decode(mac_b64).ok()?;
    let mut m = mac(key);
    m.update(&payload);
    // Constant-time comparison inside verify_slice.
    m.verify_slice(&tag).ok()?;
    serde_json::from_slice(&payload).ok()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessClaims {
    /// File manager username.
    pub u: String,
    /// The user's token_version at issue time.
    pub tv: u32,
    /// Telegram user id the session was opened with.
    pub tg: i64,
    pub iat: u64,
    pub exp: u64,
}

pub fn issue_access(keys: &TokenKeys, claims: &AccessClaims) -> String {
    format!("{ACCESS_PREFIX}{}", sign(&keys.access, claims))
}

/// Signature and expiry only; the caller re-checks the user and token_version.
pub fn verify_access(keys: &TokenKeys, token: &str, now: u64) -> Option<AccessClaims> {
    let claims: AccessClaims = verify(&keys.access, token.strip_prefix(ACCESS_PREFIX)?)?;
    (claims.exp > now).then_some(claims)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Disposition {
    /// Content-Disposition: attachment (download dialog).
    #[serde(rename = "a")]
    Attachment,
    /// Inline display for safe media types only (previews, streaming).
    #[serde(rename = "i")]
    Inline,
}

/// A signed link to exactly one file of one user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlClaims {
    pub u: String,
    pub tv: u32,
    pub z: Zone,
    pub c: String,
    pub p: String,
    pub n: String,
    pub d: Disposition,
    pub exp: u64,
}

pub fn sign_url(keys: &TokenKeys, claims: &UrlClaims) -> String {
    sign(&keys.url, claims)
}

pub fn verify_url(keys: &TokenKeys, token: &str, now: u64) -> Option<UrlClaims> {
    let claims: UrlClaims = verify(&keys.url, token)?;
    (claims.exp > now).then_some(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> TokenKeys {
        TokenKeys::derive(&[1u8; 64])
    }

    fn access(exp: u64) -> AccessClaims {
        AccessClaims {
            u: "anna".into(),
            tv: 3,
            tg: 42,
            iat: 100,
            exp,
        }
    }

    #[test]
    fn access_roundtrip_and_expiry() {
        let token = issue_access(&keys(), &access(1000));
        assert!(token.starts_with("v1."));
        assert_eq!(verify_access(&keys(), &token, 999), Some(access(1000)));
        assert_eq!(verify_access(&keys(), &token, 1000), None);
    }

    #[test]
    fn tampering_and_wrong_keys_are_rejected() {
        let token = issue_access(&keys(), &access(1000));
        let other = TokenKeys::derive(&[2u8; 64]);
        assert_eq!(verify_access(&other, &token, 1), None);

        // Swap the payload for another user's, keep the old MAC.
        let (_, tag) = token["v1.".len()..].split_once('.').unwrap();
        let forged_payload = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&AccessClaims {
                u: "admin".into(),
                ..access(1000)
            })
            .unwrap(),
        );
        assert_eq!(
            verify_access(&keys(), &format!("v1.{forged_payload}.{tag}"), 1),
            None
        );
        assert_eq!(verify_access(&keys(), "v1.garbage", 1), None);
        assert_eq!(verify_access(&keys(), &token[3..], 1), None); // no prefix
    }

    #[test]
    fn purposes_use_different_keys() {
        // An access token must never be accepted as a download link and vice versa.
        let k = keys();
        let access_body = issue_access(&k, &access(1000))[3..].to_string();
        assert_eq!(verify_url(&k, &access_body, 1), None);
        let url = sign_url(
            &k,
            &UrlClaims {
                u: "anna".into(),
                tv: 3,
                z: Zone::My,
                c: "Фото".into(),
                p: "".into(),
                n: "a.jpg".into(),
                d: Disposition::Inline,
                exp: 1000,
            },
        );
        assert!(verify_url(&k, &url, 999).is_some());
        assert_eq!(verify_access(&k, &format!("v1.{url}"), 1), None);
        assert!(verify_url(&k, &url, 1000).is_none());
    }
}
