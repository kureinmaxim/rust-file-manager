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

#[derive(Clone, Serialize, Deserialize)]
pub struct User {
    pub username: String,
    pub password_hash: String,
    pub created_at: u64,
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

        // Reload from disk: data survives a restart.
        let store2 = UserStore::load(path).unwrap();
        assert!(store2.user_exists("alice"));
        store2.remove_user("alice").unwrap();
        assert!(!store2.user_exists("alice"));
    }
}
