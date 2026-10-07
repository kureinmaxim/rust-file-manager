//! Verification of Telegram Mini App `initData` without the bot token.
//!
//! Since Bot API 8.0 `initData` carries an Ed25519 `signature` made by
//! Telegram. Third parties check it with Telegram's published public key and
//! the numeric bot id (core.telegram.org/bots/webapps, "Validating data for
//! Third-Party Use"):
//!
//! ```text
//! <bot_id>:WebAppData\n<key>=<value>\n<key>=<value>…   (sorted by key,
//!                                                       without hash and signature)
//! ```
//!
//! Keeping the bot token out of this service means a compromise of the file
//! manager does not hand over the admin bot.

use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;

use crate::config::TelegramConfig;

/// Telegram's public keys for third-party validation (production and test
/// environment), as published in the Mini Apps documentation.
const TELEGRAM_PUBLIC_KEY_PROD: [u8; 32] =
    hex32("e7bf03a2fa4602af4580703d88dda5bb59f32ed8b02a56c187fe7d34caed242d");
const TELEGRAM_PUBLIC_KEY_TEST: [u8; 32] =
    hex32("40055058a4ee38156a06562e52eece92a771bcd8346a8c4615cb7376eddf72ec");

/// Clock skew tolerated for `auth_date` in the future.
const FUTURE_SKEW_SECS: u64 = 60;
/// initData is a query string of a handful of fields; anything bigger is noise.
const MAX_INIT_DATA_BYTES: usize = 8 * 1024;

const fn hex_val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => panic!("invalid hex"),
    }
}

const fn hex32(s: &str) -> [u8; 32] {
    let bytes = s.as_bytes();
    assert!(bytes.len() == 64);
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = hex_val(bytes[2 * i]) << 4 | hex_val(bytes[2 * i + 1]);
        i += 1;
    }
    out
}

/// Public key used to verify initData for this configuration.
pub fn public_key(config: &TelegramConfig) -> VerifyingKey {
    let bytes = config.dev_public_key.unwrap_or(if config.test_env {
        TELEGRAM_PUBLIC_KEY_TEST
    } else {
        TELEGRAM_PUBLIC_KEY_PROD
    });
    VerifyingKey::from_bytes(&bytes).expect("Telegram public key is a valid Ed25519 point")
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct TgUser {
    pub id: i64,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub language_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitData {
    pub user: TgUser,
    pub auth_date: u64,
    /// `startapp` parameter of a direct link (`inv_<token>`, `upload`, …);
    /// trustworthy because it is covered by the signature.
    pub start_param: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitDataError {
    Malformed,
    MissingSignature,
    BadSignature,
    Expired,
    NoUser,
}

impl InitDataError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Malformed | Self::NoUser => {
                "Не удалось прочитать данные Telegram. Откройте приложение заново."
            }
            Self::MissingSignature => {
                "Клиент Telegram не передал подпись. Обновите Telegram до последней версии."
            }
            Self::BadSignature => "Данные Telegram не прошли проверку подписи.",
            Self::Expired => "Сеанс Telegram устарел. Закройте и откройте приложение заново.",
        }
    }
}

/// Verify raw `Telegram.WebApp.initData` and return the signed fields.
pub fn verify_init_data(
    raw: &str,
    bot_id: i64,
    key: &VerifyingKey,
    now: u64,
    max_age: u64,
) -> Result<InitData, InitDataError> {
    if raw.is_empty() || raw.len() > MAX_INIT_DATA_BYTES {
        return Err(InitDataError::Malformed);
    }
    let mut pairs: Vec<(String, String)> =
        serde_urlencoded::from_str(raw).map_err(|_| InitDataError::Malformed)?;
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    if pairs.windows(2).any(|w| w[0].0 == w[1].0) {
        return Err(InitDataError::Malformed);
    }

    let mut signature = None;
    let mut lines = Vec::with_capacity(pairs.len());
    for (key, value) in &pairs {
        match key.as_str() {
            "signature" => signature = Some(value.as_str()),
            "hash" => {}
            _ => lines.push(format!("{key}={value}")),
        }
    }
    let signature = signature.ok_or(InitDataError::MissingSignature)?;
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature.trim_end_matches('='))
        .map_err(|_| InitDataError::BadSignature)?;
    let signature = Signature::from_slice(&signature).map_err(|_| InitDataError::BadSignature)?;
    let check_string = format!("{bot_id}:WebAppData\n{}", lines.join("\n"));
    key.verify_strict(check_string.as_bytes(), &signature)
        .map_err(|_| InitDataError::BadSignature)?;

    // Only fields covered by the signature are read below.
    let field = |name: &str| {
        pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    let auth_date: u64 = field("auth_date")
        .and_then(|v| v.parse().ok())
        .ok_or(InitDataError::Malformed)?;
    if auth_date > now + FUTURE_SKEW_SECS || now.saturating_sub(auth_date) > max_age {
        return Err(InitDataError::Expired);
    }
    let user: TgUser = field("user")
        .and_then(|v| serde_json::from_str(v).ok())
        .ok_or(InitDataError::NoUser)?;
    if user.id <= 0 {
        return Err(InitDataError::NoUser);
    }
    Ok(InitData {
        user,
        auth_date,
        start_param: field("start_param")
            .map(str::to_string)
            .filter(|s| !s.is_empty()),
    })
}

#[cfg(test)]
pub(crate) mod test_support {
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    pub const BOT_ID: i64 = 7_000_000_001;

    pub fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    /// Build initData the way Telegram does, signed with the test key.
    pub fn init_data(fields: &[(&str, &str)]) -> String {
        let mut sorted: Vec<_> = fields.to_vec();
        sorted.sort();
        let check = format!(
            "{BOT_ID}:WebAppData\n{}",
            sorted
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(signing_key().sign(check.as_bytes()).to_bytes());
        let mut all: Vec<(&str, &str)> = fields.to_vec();
        all.push(("hash", "0000"));
        all.push(("signature", &signature));
        serde_urlencoded::to_string(&all).unwrap()
    }

    pub fn user_json(id: i64, username: &str) -> String {
        format!(r#"{{"id":{id},"first_name":"Анна","username":"{username}","language_code":"ru"}}"#)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    const NOW: u64 = 1_791_380_000;

    fn verify(raw: &str) -> Result<InitData, InitDataError> {
        verify_init_data(raw, BOT_ID, &signing_key().verifying_key(), NOW, 3600)
    }

    fn fields(user: &str) -> Vec<(&'static str, String)> {
        vec![
            ("auth_date", (NOW - 10).to_string()),
            ("query_id", "AAHdF6IQAAAAAN0XohDhrOrc".into()),
            ("start_param", "inv_abc".into()),
            ("user", user.to_string()),
        ]
    }

    fn build(f: &[(&'static str, String)]) -> String {
        let refs: Vec<(&str, &str)> = f.iter().map(|(k, v)| (*k, v.as_str())).collect();
        init_data(&refs)
    }

    #[test]
    fn production_and_test_keys_are_valid_points() {
        assert!(VerifyingKey::from_bytes(&TELEGRAM_PUBLIC_KEY_PROD).is_ok());
        assert!(VerifyingKey::from_bytes(&TELEGRAM_PUBLIC_KEY_TEST).is_ok());
    }

    #[test]
    fn accepts_valid_init_data() {
        let data = verify(&build(&fields(&user_json(42, "anna_k")))).unwrap();
        assert_eq!(data.user.id, 42);
        assert_eq!(data.user.username.as_deref(), Some("anna_k"));
        assert_eq!(data.user.first_name, "Анна");
        assert_eq!(data.start_param.as_deref(), Some("inv_abc"));
    }

    #[test]
    fn rejects_tampered_field() {
        let raw = build(&fields(&user_json(42, "anna_k")));
        let tampered = raw.replace("%22id%22%3A42", "%22id%22%3A43");
        assert_ne!(raw, tampered);
        assert_eq!(verify(&tampered), Err(InitDataError::BadSignature));
        let swapped = raw.replace("inv_abc", "inv_xyz");
        assert_eq!(verify(&swapped), Err(InitDataError::BadSignature));
    }

    #[test]
    fn rejects_other_bot_and_other_key() {
        let raw = build(&fields(&user_json(42, "anna_k")));
        let other_key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]).verifying_key();
        assert_eq!(
            verify_init_data(&raw, BOT_ID + 1, &signing_key().verifying_key(), NOW, 3600),
            Err(InitDataError::BadSignature)
        );
        assert_eq!(
            verify_init_data(&raw, BOT_ID, &other_key, NOW, 3600),
            Err(InitDataError::BadSignature)
        );
    }

    #[test]
    fn rejects_expired_and_future_auth_date() {
        let mut old = fields(&user_json(42, "a"));
        old[0].1 = (NOW - 3601).to_string();
        assert_eq!(verify(&build(&old)), Err(InitDataError::Expired));
        let mut future = fields(&user_json(42, "a"));
        future[0].1 = (NOW + 600).to_string();
        assert_eq!(verify(&build(&future)), Err(InitDataError::Expired));
    }

    #[test]
    fn rejects_missing_signature_duplicates_and_garbage() {
        let raw = build(&fields(&user_json(42, "a")));
        let without: String = raw
            .split('&')
            .filter(|p| !p.starts_with("signature="))
            .collect::<Vec<_>>()
            .join("&");
        assert_eq!(verify(&without), Err(InitDataError::MissingSignature));
        assert_eq!(
            verify(&format!("{raw}&auth_date=1")),
            Err(InitDataError::Malformed)
        );
        assert_eq!(verify(""), Err(InitDataError::Malformed));
        assert_eq!(verify(&"a=b&".repeat(5000)), Err(InitDataError::Malformed));
    }

    #[test]
    fn requires_a_user() {
        let no_user = vec![("auth_date", (NOW - 1).to_string())];
        assert_eq!(verify(&build(&no_user)), Err(InitDataError::NoUser));
        let bad_user = fields(r#"{"id":-5}"#);
        assert_eq!(verify(&build(&bad_user)), Err(InitDataError::NoUser));
    }
}
