use std::env;
use std::path::PathBuf;

use actix_web::cookie::Key;
use base64::Engine;

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8080";
const DEFAULT_UPLOAD_DIR: &str = "uploads";
const DEFAULT_USERS_FILE: &str = "users.json";
const DEFAULT_MAX_FILE_SIZE_MB: usize = 200;

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
            cookie_secure: env::var("COOKIE_SECURE")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        })
    }

    /// Session signing key: `SESSION_SECRET` (base64, >= 64 bytes decoded) or a
    /// random key per start (sessions then reset on restart).
    pub fn session_key(&self) -> Key {
        match env::var("SESSION_SECRET") {
            Ok(b64) => match base64::engine::general_purpose::STANDARD.decode(b64.trim()) {
                Ok(bytes) if bytes.len() >= 64 => Key::from(&bytes),
                Ok(_) => {
                    tracing::warn!("SESSION_SECRET decodes to fewer than 64 bytes; using a random key");
                    Key::generate()
                }
                Err(e) => {
                    tracing::warn!("SESSION_SECRET is not valid base64 ({e}); using a random key");
                    Key::generate()
                }
            },
            Err(_) => {
                tracing::info!("SESSION_SECRET not set; sessions will reset on every restart");
                Key::generate()
            }
        }
    }
}
