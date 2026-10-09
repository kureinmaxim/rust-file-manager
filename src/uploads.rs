//! Resumable chunked uploads for the Mini App.
//!
//! Files arrive in chunks (default 8 MB) so a single request stays under the
//! Cloudflare 100 MB limit and a dropped mobile connection only costs one
//! chunk. State lives on disk in `UPLOAD_DIR/.staging/<user>/`:
//! `<id>.json` (metadata) and `<id>.part` (bytes received so far). The offset
//! is simply the length of the `.part` file, so uploads survive restarts.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use actix_web::http::StatusCode;
use actix_web::{delete, get, post, put, web, HttpRequest, HttpResponse};
use futures_util::StreamExt;
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::{finish_upload, upload_category, ApiUser};
use crate::config::AppConfig;
use crate::paths::RelPath;
use crate::reply::{self, now_secs};
use crate::storage::{exact_file_name, folder_dir, StorageError, Zone, STAGING_DIR};
use crate::users::UserStore;

/// Unfinished uploads per user; more must be finished or cancelled first.
const MAX_OPEN_UPLOADS: usize = 4;
/// Staged uploads untouched for this long are removed.
pub const STAGING_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const ID_LEN: usize = 32;

/// Upload ids currently receiving a chunk: one writer per upload.
#[derive(Default)]
pub struct UploadLocks(Mutex<HashSet<String>>);

struct LockGuard<'a> {
    locks: &'a UploadLocks,
    id: String,
}

impl UploadLocks {
    fn try_lock(&self, id: &str) -> Option<LockGuard<'_>> {
        let mut set = self.0.lock().expect("upload locks");
        set.insert(id.to_string()).then(|| LockGuard {
            locks: self,
            id: id.to_string(),
        })
    }
}

impl Drop for LockGuard<'_> {
    fn drop(&mut self) {
        self.locks.0.lock().expect("upload locks").remove(&self.id);
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct UploadMeta {
    id: String,
    username: String,
    scope: Zone,
    category: String,
    path: String,
    name: String,
    size: u64,
    created_at: u64,
    /// «Обмен»: owner of the exchange this upload goes to (then `scope`,
    /// `category` and `path` are unused).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exchange: Option<String>,
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(create)
        .service(list)
        .service(status)
        .service(put_chunk)
        .service(complete)
        .service(cancel);
}

fn user_dir(config: &AppConfig, username: &str) -> PathBuf {
    config.upload_dir.join(STAGING_DIR).join(username)
}

fn valid_id(id: &str) -> bool {
    id.len() == ID_LEN && id.chars().all(|c| c.is_ascii_alphanumeric())
}

fn not_found() -> HttpResponse {
    reply::error(
        StatusCode::NOT_FOUND,
        "upload_not_found",
        "Загрузка не найдена или устарела — начните заново",
    )
}

/// Metadata of the user's own upload; another user's id is "not found".
fn load(config: &AppConfig, user: &ApiUser, id: &str) -> Option<(UploadMeta, PathBuf)> {
    if !valid_id(id) {
        return None;
    }
    let dir = user_dir(config, &user.username);
    let meta: UploadMeta =
        serde_json::from_slice(&fs::read(dir.join(format!("{id}.json"))).ok()?).ok()?;
    (meta.username == user.username).then(|| (meta, dir.join(format!("{id}.part"))))
}

fn offset(part: &Path) -> u64 {
    fs::metadata(part).map(|m| m.len()).unwrap_or(0)
}

fn describe(meta: &UploadMeta, part: &Path) -> serde_json::Value {
    match &meta.exchange {
        Some(owner) => json!({
            "id": meta.id, "name": meta.name, "size": meta.size, "offset": offset(part),
            "scope": "exchange", "with": owner, "category": "", "path": "",
        }),
        None => json!({
            "id": meta.id, "name": meta.name, "size": meta.size, "offset": offset(part),
            "scope": meta.scope, "category": meta.category, "path": meta.path,
        }),
    }
}

#[derive(Deserialize)]
struct CreateBody {
    scope: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    path: String,
    name: String,
    size: u64,
    /// For `scope: "exchange"`: whose exchange (the admin names a user; a user
    /// may leave it empty for their own).
    #[serde(default)]
    with: String,
}

// Older nginx configurations redirected this URL and WebViews may cache it.
// Both paths stay inside the same authenticated API scope.
#[actix_web::routes]
#[post("/uploads")]
#[post("/uploads/")]
async fn create(
    body: web::Json<CreateBody>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    let result = (|| -> Result<HttpResponse, StorageError> {
        let name = exact_file_name(&body.name)?.to_string();
        let (zone, category, rel, exchange) = if body.scope == "exchange" {
            let owner = crate::exchange::check_upload_target(&store, &user, &body.with)?;
            (Zone::My, String::new(), RelPath::root(), Some(owner))
        } else {
            let zone = Zone::parse(&body.scope).ok_or(StorageError::Zone)?;
            let category = upload_category(body.category.as_deref(), &name)?;
            let rel = RelPath::parse(&body.path).map_err(StorageError::Path)?;
            // Validate the destination now (symlinks, category), not only at the end.
            folder_dir(&config, zone, &user.username, &category, &rel)?;
            (zone, category, rel, None)
        };
        if body.size > config.max_chunked_file_size {
            return Ok(reply::error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                format!(
                    "Файл больше лимита {}",
                    crate::storage::format_bytes(config.max_chunked_file_size)
                ),
            ));
        }
        let dir = user_dir(&config, &user.username);
        fs::create_dir_all(&dir)?;
        let open = fs::read_dir(&dir)?
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .count();
        if open >= MAX_OPEN_UPLOADS {
            return Ok(reply::error(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_uploads",
                "Слишком много незавершённых загрузок — дождитесь окончания или отмените лишние",
            ));
        }
        let id: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(ID_LEN)
            .map(char::from)
            .collect();
        let meta = UploadMeta {
            id: id.clone(),
            username: user.username.clone(),
            scope: zone,
            category,
            path: rel.to_string(),
            name,
            size: body.size,
            created_at: now_secs(),
            exchange,
        };
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(format!("{id}.part")))?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(format!("{id}.json")))?;
        file.write_all(&serde_json::to_vec(&meta).expect("meta serializes"))?;
        let part = dir.join(format!("{id}.part"));
        let mut body = describe(&meta, &part);
        body["chunk_size"] = json!(config.upload_chunk_size);
        Ok(HttpResponse::Created().json({
            body["success"] = json!(true);
            body
        }))
    })();
    result.unwrap_or_else(reply::storage)
}

/// Unfinished uploads of this user — the app matches them by name and size
/// to resume when the same file is picked again.
#[actix_web::routes]
#[get("/uploads")]
#[get("/uploads/")]
async fn list(user: web::ReqData<ApiUser>, config: web::Data<AppConfig>) -> HttpResponse {
    let dir = user_dir(&config, &user.username);
    let uploads: Vec<_> = fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    let id = name.strip_suffix(".json")?;
                    let (meta, part) = load(&config, &user, id)?;
                    Some(describe(&meta, &part))
                })
                .collect()
        })
        .unwrap_or_default();
    reply::ok(json!({ "uploads": uploads }))
}

#[get("/uploads/{id}")]
async fn status(
    id: web::Path<String>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
) -> HttpResponse {
    match load(&config, &user, &id) {
        Some((meta, part)) => reply::ok(describe(&meta, &part)),
        None => not_found(),
    }
}

fn offset_mismatch(current: u64) -> HttpResponse {
    reply::error_with(
        StatusCode::CONFLICT,
        json!({
            "code": "offset_mismatch",
            "message": "Сервер ждёт другую часть файла",
            "offset": current,
        }),
    )
}

#[put("/uploads/{id}")]
async fn put_chunk(
    req: HttpRequest,
    id: web::Path<String>,
    mut payload: web::Payload,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    locks: web::Data<UploadLocks>,
) -> HttpResponse {
    let Some((meta, part)) = load(&config, &user, &id) else {
        return not_found();
    };
    let Some(_guard) = locks.try_lock(&id) else {
        return reply::error(StatusCode::CONFLICT, "busy", "Эта часть уже загружается");
    };
    let Some(claimed) = req
        .headers()
        .get("Upload-Offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return reply::bad_request("Нужен заголовок Upload-Offset");
    };
    let current = offset(&part);
    if claimed != current {
        return offset_mismatch(current);
    }
    let mut file = match fs::OpenOptions::new().append(true).open(&part) {
        Ok(f) => f,
        Err(e) => return reply::storage(e.into()),
    };
    let limit = (config.upload_chunk_size as u64).min(meta.size - current.min(meta.size));
    let mut written = 0u64;
    let mut failure = None;
    while let Some(chunk) = payload.next().await {
        let data = match chunk {
            Ok(d) => d,
            Err(_) => {
                failure = Some(reply::error(
                    StatusCode::BAD_REQUEST,
                    "interrupted",
                    "Передача части прервалась",
                ));
                break;
            }
        };
        written += data.len() as u64;
        if written > limit {
            failure = Some(reply::error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "chunk_too_large",
                "Часть больше допустимого размера",
            ));
            break;
        }
        if let Err(e) = file.write_all(&data) {
            failure = Some(reply::storage(e.into()));
            break;
        }
    }
    if let Some(resp) = failure {
        // Roll back to the last confirmed offset so a retry starts cleanly.
        let _ = file.set_len(current);
        return resp;
    }
    if let Err(e) = file.flush() {
        let _ = file.set_len(current);
        return reply::storage(e.into());
    }
    reply::ok(json!({ "offset": current + written, "size": meta.size }))
}

#[post("/uploads/{id}/complete")]
async fn complete(
    id: web::Path<String>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    events: web::Data<crate::exchange::Events>,
    locks: web::Data<UploadLocks>,
) -> HttpResponse {
    let Some((meta, part)) = load(&config, &user, &id) else {
        return not_found();
    };
    let Some(_guard) = locks.try_lock(&id) else {
        return reply::error(
            StatusCode::CONFLICT,
            "busy",
            "Загрузка ещё принимает данные",
        );
    };
    let current = offset(&part);
    if current != meta.size {
        return offset_mismatch(current);
    }
    if let Some(owner) = &meta.exchange {
        return match crate::exchange::finish_upload(
            &config, &store, &events, &user, owner, &part, &meta.name,
        ) {
            Ok(item) => {
                let _ =
                    fs::remove_file(user_dir(&config, &user.username).join(format!("{id}.json")));
                tracing::info!(user = %user.username, exchange = %owner, size = meta.size, "file sent to exchange (Mini App)");
                reply::ok(json!({ "item": item }))
            }
            Err(e) => reply::storage(e),
        };
    }
    let rel = match RelPath::parse(&meta.path) {
        Ok(r) => r,
        Err(e) => return reply::bad_request(e),
    };
    match finish_upload(
        &config,
        meta.scope,
        &user.username,
        &meta.category,
        &rel,
        &part,
        &meta.name,
    ) {
        Ok(item) => {
            let _ = fs::remove_file(user_dir(&config, &user.username).join(format!("{id}.json")));
            tracing::info!(
                user = %user.username, scope = meta.scope.as_str(), category = %meta.category,
                size = meta.size, "file uploaded (Mini App)"
            );
            reply::ok(json!({ "item": item }))
        }
        Err(e) => reply::storage(e),
    }
}

#[delete("/uploads/{id}")]
async fn cancel(
    id: web::Path<String>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    locks: web::Data<UploadLocks>,
) -> HttpResponse {
    let Some((_, part)) = load(&config, &user, &id) else {
        return not_found();
    };
    let Some(_guard) = locks.try_lock(&id) else {
        return reply::error(StatusCode::CONFLICT, "busy", "Эта часть ещё загружается");
    };
    let _ = fs::remove_file(&part);
    let _ = fs::remove_file(user_dir(&config, &user.username).join(format!("{id}.json")));
    reply::ok(json!({ "message": "Загрузка отменена" }))
}

/// Remove staged uploads older than `ttl` (by modification time). Returns the
/// number of files removed.
pub fn cleanup_staging(upload_dir: &Path, ttl: Duration) -> usize {
    let mut removed = 0;
    let Ok(users) = fs::read_dir(upload_dir.join(STAGING_DIR)) else {
        return 0;
    };
    let cutoff = SystemTime::now() - ttl;
    for user in users.flatten() {
        let Ok(entries) = fs::read_dir(user.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t < cutoff);
            if stale && fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}
