use std::env;
use std::path::PathBuf;

use actix_web::cookie::Key;
use base64::Engine;

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8080";
const DEFAULT_UPLOAD_DIR: &str = "uploads";
const DEFAULT_USERS_FILE: &str = "users.json";
const DEFAULT_MAX_FILE_SIZE_MB: usize = 200;
const DEFAULT_MAX_CHUNKED_FILE_SIZE_MB: u64 = 4096;
const DEFAULT_UPLOAD_CHUNK_SIZE_MB: usize = 8;
const DEFAULT_INIT_DATA_MAX_AGE_SECS: u64 = 3600;
/// Shortest accepted INTERNAL_API_TOKEN: the internal API trusts the bot with
/// any Telegram id, so the token must not be guessable.
const MIN_INTERNAL_TOKEN_LEN: usize = 32;

/// Telegram Mini App settings. The bot token is deliberately absent: initData
/// is verified with Telegram's Ed25519 public key and the numeric bot id.
#[derive(Clone)]
pub struct TelegramConfig {
    pub bot_id: i64,
    pub bot_username: String,
    pub test_env: bool,
    pub init_data_max_age: u64,
    /// Debug builds only: replace Telegram's public key to test locally.
    pub dev_public_key: Option<[u8; 32]>,
}

/// Runtime configuration, loaded from environment variables (and `.env` if present).
#[derive(Clone)]
pub struct AppConfig {
    pub bind_addr: String,
    pub upload_dir: PathBuf,
    pub users_file: PathBuf,
    pub max_file_size: usize,
    pub admin_username: String,
    pub admin_password_hash: String,
    pub cookie_secure: bool,
    /// Telegram Mini App; `None` when MINIAPP_ENABLED is off.
    pub telegram: Option<TelegramConfig>,
    /// Public HTTPS origin, used to build invite links for the bot.
    pub public_base_url: Option<String>,
    /// Second listener for the bot's internal API (with its bearer token).
    pub internal_bind_addr: Option<String>,
    pub internal_api_token: Option<String>,
    pub max_chunked_file_size: u64,
    pub upload_chunk_size: usize,
    /// Session secret material (cookie key and derived token keys). Not Debug.
    pub secret: Vec<u8>,
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn env_number<T: std::str::FromStr>(name: &str, default: T) -> Result<T, String> {
    match env::var(name) {
        Ok(v) if !v.trim().is_empty() => v
            .trim()
            .parse::<T>()
            .map_err(|_| format!("{name} must be a number, got: {v}")),
        _ => Ok(default),
    }
}

fn non_empty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse_hex32(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(out)
}

fn telegram_from_env() -> Result<Option<TelegramConfig>, String> {
    if !env_flag("MINIAPP_ENABLED") {
        return Ok(None);
    }
    let bot_id = non_empty("TELEGRAM_BOT_ID")
        .ok_or("MINIAPP_ENABLED=true requires TELEGRAM_BOT_ID (the number before ':' in the bot token)")?
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or("TELEGRAM_BOT_ID must be a positive number")?;
    let bot_username = non_empty("TELEGRAM_BOT_USERNAME")
        .map(|u| u.trim_start_matches('@').to_string())
        .ok_or("MINIAPP_ENABLED=true requires TELEGRAM_BOT_USERNAME")?;
    if !(5..=32).contains(&bot_username.len())
        || !bot_username.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("TELEGRAM_BOT_USERNAME must be 5-32 latin letters, digits or '_'".into());
    }
    // WARNING: a replaced public key accepts initData signed by anyone holding
    // the matching private key. Only debug builds read it, for local testing;
    // release binaries always use Telegram's published keys.
    #[cfg(debug_assertions)]
    let dev_public_key = match non_empty("TG_DEV_PUBLIC_KEY") {
        Some(hex) => Some(parse_hex32(&hex).ok_or("TG_DEV_PUBLIC_KEY must be 64 hex chars")?),
        None => None,
    };
    #[cfg(not(debug_assertions))]
    let dev_public_key = None;
    Ok(Some(TelegramConfig {
        bot_id,
        bot_username,
        test_env: env_flag("TG_TEST_ENV"),
        init_data_max_age: env_number("TG_INIT_DATA_MAX_AGE_SECS", DEFAULT_INIT_DATA_MAX_AGE_SECS)?,
        dev_public_key,
    }))
}

/// Session secret: `SESSION_SECRET` (base64, >= 64 bytes decoded) or 64
/// random bytes per start (sessions and Mini App tokens then reset on restart).
fn load_secret() -> Vec<u8> {
    match env::var("SESSION_SECRET") {
        Ok(b64) => match base64::engine::general_purpose::STANDARD.decode(b64.trim()) {
            Ok(bytes) if bytes.len() >= 64 => return bytes,
            Ok(_) => tracing::warn!("SESSION_SECRET decodes to fewer than 64 bytes; using a random key"),
            Err(e) => tracing::warn!("SESSION_SECRET is not valid base64 ({e}); using a random key"),
        },
        Err(_) => tracing::info!("SESSION_SECRET not set; sessions will reset on every restart"),
    }
    (0..64).map(|_| rand::random::<u8>()).collect()
}

impl AppConfig {
    pub fn from_env() -> Result<Self, String> {
        let admin_password_hash = env::var("ADMIN_PASSWORD_HASH").map_err(|_| {
            "ADMIN_PASSWORD_HASH is not set.\n\
             Generate one with:  echo 'your-password' | rust-file-manager hash-password\n\
             then export it or put it into a .env file."
                .to_string()
        })?;

        // dotenvy expands $-sequences in unquoted/double-quoted .env values,
        // which truncates bcrypt hashes; fail fast instead of rejecting every login.
        if !admin_password_hash.starts_with("$2") {
            return Err(format!(
                "ADMIN_PASSWORD_HASH does not look like a bcrypt hash (got \"{}...\").\n\
                 If it is set in a .env file, wrap the value in SINGLE quotes:\n\
                 ADMIN_PASSWORD_HASH='$2b$12$...'\n\
                 (without quotes, $-sequences are expanded as variables and the hash is corrupted)",
                admin_password_hash.chars().take(8).collect::<String>()
            ));
        }

        let max_file_size_mb = match env::var("MAX_FILE_SIZE_MB") {
            Ok(v) => v
                .parse::<usize>()
                .map_err(|_| format!("MAX_FILE_SIZE_MB must be a number, got: {v}"))?,
            Err(_) => DEFAULT_MAX_FILE_SIZE_MB,
        };

        let internal_api_token = non_empty("INTERNAL_API_TOKEN");
        if non_empty("INTERNAL_BIND_ADDR").is_some()
            && internal_api_token
                .as_ref()
                .is_none_or(|t| t.len() < MIN_INTERNAL_TOKEN_LEN)
        {
            return Err(format!(
                "INTERNAL_BIND_ADDR is set, so INTERNAL_API_TOKEN must be at least {MIN_INTERNAL_TOKEN_LEN} characters.\n\
                 Generate one with:  openssl rand -base64 48 | tr -d '\\n'"
            ));
        }

        Ok(Self {
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_string()),
            upload_dir: PathBuf::from(
                env::var("UPLOAD_DIR").unwrap_or_else(|_| DEFAULT_UPLOAD_DIR.to_string()),
            ),
            users_file: PathBuf::from(
                env::var("USERS_FILE").unwrap_or_else(|_| DEFAULT_USERS_FILE.to_string()),
            ),
            max_file_size: max_file_size_mb * 1024 * 1024,
            admin_username: env::var("ADMIN_USERNAME").unwrap_or_else(|_| "admin".to_string()),
            admin_password_hash,
            cookie_secure: env_flag("COOKIE_SECURE"),
            telegram: telegram_from_env()?,
            public_base_url: non_empty("PUBLIC_BASE_URL").map(|u| u.trim_end_matches('/').to_string()),
            internal_bind_addr: non_empty("INTERNAL_BIND_ADDR"),
            internal_api_token,
            max_chunked_file_size: env_number("MAX_CHUNKED_FILE_SIZE_MB", DEFAULT_MAX_CHUNKED_FILE_SIZE_MB)?
                * 1024
                * 1024,
            upload_chunk_size: env_number("UPLOAD_CHUNK_SIZE_MB", DEFAULT_UPLOAD_CHUNK_SIZE_MB)?
                .clamp(1, 64)
                * 1024
                * 1024,
            secret: load_secret(),
        })
    }

    /// Cookie signing key derived from the session secret.
    pub fn session_key(&self) -> Key {
        Key::from(&self.secret)
    }
}
