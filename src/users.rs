use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::{Deserialize, Serialize};

/// Invite links expire after this many seconds (7 days).
const INVITE_TTL_SECS: u64 = 7 * 24 * 60 * 60;
const TOKEN_LEN: usize = 43; // ~256 bits of alphanumeric entropy

/// Password hash of accounts created from Telegram: not a bcrypt hash, so
/// password login always fails until the user sets a password.
const NO_PASSWORD: &str = "!";

fn is_zero(v: &u32) -> bool {
    *v == 0
}

#[derive(Clone, Serialize, Deserialize)]
pub struct User {
    pub username: String,
    pub password_hash: String,
    pub created_at: u64,
    /// Telegram account linked to this user (Mini App login).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_id: Option<i64>,
    /// Bumped to revoke every Mini App token and signed link of the user.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub token_version: u32,
}

/// The admin account lives in the environment, not in `users`; its Telegram
/// link and token version are kept here.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct AdminProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_id: Option<i64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub token_version: u32,
}

/// A user or the admin, as seen by the Mini App and the internal API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub username: String,
    pub is_admin: bool,
    pub telegram_id: Option<i64>,
    pub token_version: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LinkError {
    NoUser,
    /// The Telegram id or the account is already linked elsewhere.
    AlreadyLinked,
    NotLinked,
    Save(String),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Invite {
    pub token: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub created_by: String,
}

#[derive(Default, Serialize, Deserialize)]
struct StoreData {
    #[serde(default)]
    users: Vec<User>,
    #[serde(default)]
    invites: Vec<Invite>,
    #[serde(default)]
    admin: AdminProfile,
}

/// Persistent user/invite registry: an in-memory copy guarded by a lock,
/// flushed to a JSON file (atomic write via rename) on every change.
pub struct UserStore {
    path: PathBuf,
    data: RwLock<StoreData>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn generate_token() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(TOKEN_LEN)
        .map(char::from)
        .collect()
}

/// Usernames become directory names, so the alphabet is restricted to
/// characters that are safe on every filesystem and in URLs.
pub fn validate_username(raw: &str) -> Result<String, String> {
    let name = raw.trim().to_lowercase();
    if name.len() < 3 || name.len() > 32 {
        return Err("Имя пользователя должно быть от 3 до 32 символов".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err("Допустимы только латинские буквы, цифры, «_» и «-»".into());
    }
    // Reserved: zone names used in URLs/paths.
    if name == "shared" || name == "my" || name == "admin" || name == "home" {
        return Err("Это имя зарезервировано".into());
    }
    Ok(name)
}

impl UserStore {
    pub fn load(path: PathBuf) -> std::io::Result<Self> {
        let data = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{} is corrupted: {e}", path.display()),
                )
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StoreData::default(),
            Err(e) => return Err(e),
        };
        Ok(Self { path, data: RwLock::new(data) })
    }

    fn save(&self, data: &StoreData) -> std::io::Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(data).expect("serialize users"))?;
        fs::rename(&tmp, &self.path)
    }

    pub fn user_exists(&self, username: &str) -> bool {
        let data = self.data.read().expect("user store lock");
        data.users.iter().any(|u| u.username == username)
    }

    pub fn verify_password(&self, username: &str, password: &str) -> bool {
        let hash = {
            let data = self.data.read().expect("user store lock");
            data.users
                .iter()
                .find(|u| u.username == username)
                .map(|u| u.password_hash.clone())
        };
        match hash {
            Some(h) => bcrypt::verify(password, &h).unwrap_or(false),
            None => false,
        }
    }

    pub fn list_users(&self) -> Vec<User> {
        let data = self.data.read().expect("user store lock");
        data.users.clone()
    }

    pub fn add_user(&self, username: &str, password: &str) -> Result<(), String> {
        let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
            .map_err(|e| format!("Ошибка хеширования пароля: {e}"))?;
        let mut data = self.data.write().expect("user store lock");
        if data.users.iter().any(|u| u.username == username) {
            return Err("Пользователь с таким именем уже существует".into());
        }
        data.users.push(User {
            username: username.to_string(),
            password_hash: hash,
            created_at: now(),
            telegram_id: None,
            token_version: 0,
        });
        self.save(&data).map_err(|e| format!("Ошибка сохранения: {e}"))
    }

    pub fn remove_user(&self, username: &str) -> Result<(), String> {
        let mut data = self.data.write().expect("user store lock");
        let before = data.users.len();
        data.users.retain(|u| u.username != username);
        if data.users.len() == before {
            return Err("Пользователь не найден".into());
        }
        self.save(&data).map_err(|e| format!("Ошибка сохранения: {e}"))
    }

    fn account_of(data: &StoreData, username: &str, admin_username: &str) -> Option<Account> {
        if username == admin_username {
            return Some(Account {
                username: username.to_string(),
                is_admin: true,
                telegram_id: data.admin.telegram_id,
                token_version: data.admin.token_version,
            });
        }
        data.users.iter().find(|u| u.username == username).map(|u| Account {
            username: u.username.clone(),
            is_admin: false,
            telegram_id: u.telegram_id,
            token_version: u.token_version,
        })
    }

    /// Account by username (the admin included).
    pub fn account(&self, username: &str, admin_username: &str) -> Option<Account> {
        let data = self.data.read().expect("user store lock");
        Self::account_of(&data, username, admin_username)
    }

    /// Account linked to a Telegram user id.
    pub fn account_by_telegram(&self, telegram_id: i64, admin_username: &str) -> Option<Account> {
        let data = self.data.read().expect("user store lock");
        if data.admin.telegram_id == Some(telegram_id) {
            return Self::account_of(&data, admin_username, admin_username);
        }
        let user = data.users.iter().find(|u| u.telegram_id == Some(telegram_id))?;
        Self::account_of(&data, &user.username, admin_username)
    }

    /// Every account linked to Telegram (for the bot's menu buttons).
    pub fn telegram_links(&self, admin_username: &str) -> Vec<Account> {
        let data = self.data.read().expect("user store lock");
        let admin = Self::account_of(&data, admin_username, admin_username);
        admin
            .into_iter()
            .chain(data.users.iter().filter_map(|u| Self::account_of(&data, &u.username, admin_username)))
            .filter(|a| a.telegram_id.is_some())
            .collect()
    }

    pub fn user_count(&self) -> usize {
        self.data.read().expect("user store lock").users.len()
    }

    /// Link a Telegram id to an account. Idempotent for the same pair; refuses
    /// to move a Telegram id or an account that is linked elsewhere.
    pub fn link_telegram(
        &self,
        username: &str,
        admin_username: &str,
        telegram_id: i64,
    ) -> Result<(), LinkError> {
        let mut data = self.data.write().expect("user store lock");
        let linked_to = if data.admin.telegram_id == Some(telegram_id) {
            Some(admin_username.to_string())
        } else {
            data.users
                .iter()
                .find(|u| u.telegram_id == Some(telegram_id))
                .map(|u| u.username.clone())
        };
        match linked_to {
            Some(owner) if owner == username => return Ok(()),
            Some(_) => return Err(LinkError::AlreadyLinked),
            None => {}
        }
        if username == admin_username {
            if data.admin.telegram_id.is_some() {
                return Err(LinkError::AlreadyLinked);
            }
            data.admin.telegram_id = Some(telegram_id);
        } else {
            let user = data
                .users
                .iter_mut()
                .find(|u| u.username == username)
                .ok_or(LinkError::NoUser)?;
            if user.telegram_id.is_some() {
                return Err(LinkError::AlreadyLinked);
            }
            user.telegram_id = Some(telegram_id);
        }
        self.save(&data).map_err(|e| LinkError::Save(format!("Ошибка сохранения: {e}")))
    }

    /// Unlink a Telegram id and revoke the account's tokens; returns the username.
    pub fn unlink_telegram(&self, telegram_id: i64, admin_username: &str) -> Result<String, LinkError> {
        let mut data = self.data.write().expect("user store lock");
        let username = if data.admin.telegram_id == Some(telegram_id) {
            data.admin.telegram_id = None;
            data.admin.token_version = data.admin.token_version.wrapping_add(1);
            admin_username.to_string()
        } else {
            let user = data
                .users
                .iter_mut()
                .find(|u| u.telegram_id == Some(telegram_id))
                .ok_or(LinkError::NotLinked)?;
            user.telegram_id = None;
            user.token_version = user.token_version.wrapping_add(1);
            user.username.clone()
        };
        self.save(&data).map_err(|e| LinkError::Save(format!("Ошибка сохранения: {e}")))?;
        Ok(username)
    }

    /// Revoke every Mini App token and signed link of an account.
    pub fn bump_token_version(&self, username: &str, admin_username: &str) -> Result<(), String> {
        let mut data = self.data.write().expect("user store lock");
        if username == admin_username {
            data.admin.token_version = data.admin.token_version.wrapping_add(1);
        } else {
            let user = data
                .users
                .iter_mut()
                .find(|u| u.username == username)
                .ok_or("Пользователь не найден")?;
            user.token_version = user.token_version.wrapping_add(1);
        }
        self.save(&data).map_err(|e| format!("Ошибка сохранения: {e}"))
    }

    /// Create an account registered from Telegram (no password yet).
    pub fn add_telegram_user(&self, username: &str, telegram_id: i64) -> Result<(), String> {
        let mut data = self.data.write().expect("user store lock");
        if data.users.iter().any(|u| u.username == username) {
            return Err("Пользователь с таким именем уже существует".into());
        }
        if data.admin.telegram_id == Some(telegram_id)
            || data.users.iter().any(|u| u.telegram_id == Some(telegram_id))
        {
            return Err("Этот Telegram уже привязан к другому аккаунту".into());
        }
        data.users.push(User {
            username: username.to_string(),
            password_hash: NO_PASSWORD.to_string(),
            created_at: now(),
            telegram_id: Some(telegram_id),
            token_version: 0,
        });
        self.save(&data).map_err(|e| format!("Ошибка сохранения: {e}"))
    }

    /// Create a single-use invite token, dropping expired ones along the way.
    pub fn create_invite(&self, created_by: &str) -> Result<Invite, String> {
        let invite = Invite {
            token: generate_token(),
            created_at: now(),
            expires_at: now() + INVITE_TTL_SECS,
            created_by: created_by.to_string(),
        };
        let mut data = self.data.write().expect("user store lock");
        let ts = now();
        data.invites.retain(|i| i.expires_at > ts);
        data.invites.push(invite.clone());
        self.save(&data).map_err(|e| format!("Ошибка сохранения: {e}"))?;
        Ok(invite)
    }

    pub fn invite_valid(&self, token: &str) -> bool {
        let data = self.data.read().expect("user store lock");
        let ts = now();
        data.invites.iter().any(|i| i.token == token && i.expires_at > ts)
    }

    /// Consume an invite token: it is removed so each link works exactly once.
    pub fn take_invite(&self, token: &str) -> Result<(), String> {
        let mut data = self.data.write().expect("user store lock");
        let ts = now();
        data.invites.retain(|i| i.expires_at > ts);
        let before = data.invites.len();
        data.invites.retain(|i| i.token != token);
        if data.invites.len() == before {
            return Err("Ссылка-приглашение недействительна или истекла".into());
        }
        self.save(&data).map_err(|e| format!("Ошибка сохранения: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_validation() {
        assert_eq!(validate_username("  Ivan_42 ").as_deref(), Ok("ivan_42"));
        assert!(validate_username("ab").is_err());
        assert!(validate_username("иван").is_err());
        assert!(validate_username("a/b/c").is_err());
        assert!(validate_username("shared").is_err());
        assert!(validate_username("..").is_err());
    }

    #[test]
    fn store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("rfm-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("users.json");
        let _ = std::fs::remove_file(&path);

        let store = UserStore::load(path.clone()).unwrap();
        store.add_user("alice", "secret").unwrap();
        assert!(store.user_exists("alice"));
        assert!(store.verify_password("alice", "secret"));
        assert!(!store.verify_password("alice", "wrong"));
        assert!(store.add_user("alice", "x").is_err());

        let invite = store.create_invite("admin").unwrap();
        assert!(store.invite_valid(&invite.token));
        store.take_invite(&invite.token).unwrap();
        assert!(!store.invite_valid(&invite.token));
        assert!(store.take_invite(&invite.token).is_err());

        // Telegram links: idempotent, exclusive, revocable.
        store.add_user("bob", "secret").unwrap();
        assert_eq!(store.link_telegram("bob", "admin", 42), Ok(()));
        assert_eq!(store.link_telegram("bob", "admin", 42), Ok(()));
        assert_eq!(store.link_telegram("alice", "admin", 42), Err(LinkError::AlreadyLinked));
        assert_eq!(store.link_telegram("bob", "admin", 43), Err(LinkError::AlreadyLinked));
        assert_eq!(store.link_telegram("nobody", "admin", 44), Err(LinkError::NoUser));
        assert_eq!(store.link_telegram("admin", "admin", 7), Ok(()));
        let bob = store.account_by_telegram(42, "admin").unwrap();
        assert_eq!((bob.username.as_str(), bob.is_admin, bob.token_version), ("bob", false, 0));
        assert!(store.account_by_telegram(7, "admin").unwrap().is_admin);
        assert_eq!(store.telegram_links("admin").len(), 2);
        assert_eq!(store.unlink_telegram(42, "admin").as_deref(), Ok("bob"));
        assert_eq!(store.account("bob", "admin").unwrap().token_version, 1);
        assert_eq!(store.unlink_telegram(42, "admin"), Err(LinkError::NotLinked));

        // Telegram-only accounts cannot log in with a password.
        store.add_telegram_user("tg_only", 99).unwrap();
        assert!(!store.verify_password("tg_only", ""));
        assert!(!store.verify_password("tg_only", "!"));
        assert!(store.add_telegram_user("other", 99).is_err());
        assert!(store.add_telegram_user("tg_only", 100).is_err());

        // Reload from disk: data survives a restart.
        let store2 = UserStore::load(path).unwrap();
        assert!(store2.user_exists("alice"));
        assert_eq!(store2.account_by_telegram(7, "admin").unwrap().username, "admin");
        assert_eq!(store2.account_by_telegram(99, "admin").unwrap().username, "tg_only");
        store2.remove_user("alice").unwrap();
        assert!(!store2.user_exists("alice"));
    }
}
