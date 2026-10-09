//! Files through the chat with the bot.
//!
//! * From chat to storage: the bot downloads an attachment from Telegram and
//!   sends it to `POST /internal/v1/files` for the Telegram account that sent
//!   it; the file lands in «Мои файлы» or «Общие файлы» like a normal upload.
//! * From storage to chat: «Отправить в чат» in the Mini App queues a job for
//!   the requesting account; the bot long-polls `GET /internal/v1/jobs`,
//!   downloads the file and sends it to that account's chat.
//!
//! Rights are checked where the request originates (the Mini App token or the
//! Telegram account the bot names); the bot only ever sends a file to the chat
//! of the account that asked for it.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use actix_files::NamedFile;
use actix_web::http::header::{ContentDisposition, DispositionType};
use actix_web::http::StatusCode;
use actix_web::{get, post, web, HttpRequest, HttpResponse};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;

use crate::api::{self, ApiUser};
use crate::categories::sanitize_file_name;
use crate::config::AppConfig;
use crate::paths::RelPath;
use crate::reply;
use crate::storage::{format_bytes, PendingUpload, StorageError, Zone, STAGING_DIR};
use crate::users::UserStore;

/// Waiting jobs, all accounts together; beyond this the Mini App gets 429.
const MAX_QUEUED: usize = 100;
/// Waiting jobs of one account.
const MAX_QUEUED_PER_USER: usize = 5;
/// A job nobody picked up in this time is dropped: the user stopped waiting.
const QUEUE_TTL: Duration = Duration::from_secs(10 * 60);
/// How long the bot may download a job it took.
const LEASE_TTL: Duration = Duration::from_secs(10 * 60);
/// Jobs handed to the bot per request: it sends them one by one anyway.
const TAKE_LIMIT: usize = 5;
/// Longest long-poll the bot may ask for.
const MAX_WAIT_SECS: u64 = 30;
const POLL_STEP: Duration = Duration::from_millis(300);

/// A file one account asked to receive in its chat with the bot.
#[derive(Clone, Debug)]
pub struct Job {
    pub id: u64,
    pub telegram_id: i64,
    pub username: String,
    pub name: String,
    pub size: u64,
    path: PathBuf,
    created: Instant,
}

#[derive(Default)]
struct JobState {
    next_id: u64,
    queue: VecDeque<Job>,
    taken: HashMap<u64, (Job, Instant)>,
}

/// In-memory queue shared by the public server (writer) and the internal API
/// (reader). Lost on restart, which only means the user taps the button again.
#[derive(Default)]
pub struct ChatJobs {
    inner: Mutex<JobState>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum QueueError {
    Full,
}

impl ChatJobs {
    pub fn push(
        &self,
        telegram_id: i64,
        username: &str,
        name: &str,
        size: u64,
        path: PathBuf,
    ) -> Result<u64, QueueError> {
        let mut state = self.inner.lock().expect("chat jobs");
        state.queue.retain(|j| j.created.elapsed() < QUEUE_TTL);
        let mine = state
            .queue
            .iter()
            .filter(|j| j.username == username)
            .count();
        if state.queue.len() >= MAX_QUEUED || mine >= MAX_QUEUED_PER_USER {
            return Err(QueueError::Full);
        }
        state.next_id += 1;
        let id = state.next_id;
        state.queue.push_back(Job {
            id,
            telegram_id,
            username: username.to_string(),
            name: name.to_string(),
            size,
            path,
            created: Instant::now(),
        });
        Ok(id)
    }

    /// Up to `limit` fresh jobs, oldest first; they move to the leased set.
    pub fn take(&self, limit: usize) -> Vec<Job> {
        let mut state = self.inner.lock().expect("chat jobs");
        state.queue.retain(|j| j.created.elapsed() < QUEUE_TTL);
        state.taken.retain(|_, (_, at)| at.elapsed() < LEASE_TTL);
        let count = limit.min(state.queue.len());
        let jobs: Vec<Job> = state.queue.drain(..count).collect();
        for job in &jobs {
            state.taken.insert(job.id, (job.clone(), Instant::now()));
        }
        jobs
    }

    fn has_queued(&self) -> bool {
        let state = self.inner.lock().expect("chat jobs");
        state.queue.iter().any(|j| j.created.elapsed() < QUEUE_TTL)
    }

    /// A job the bot took and may still download.
    pub fn leased(&self, id: u64) -> Option<Job> {
        let state = self.inner.lock().expect("chat jobs");
        state
            .taken
            .get(&id)
            .filter(|(_, at)| at.elapsed() < LEASE_TTL)
            .map(|(job, _)| job.clone())
    }

    pub fn finish(&self, id: u64) -> Option<Job> {
        let mut state = self.inner.lock().expect("chat jobs");
        state.taken.remove(&id).map(|(job, _)| job)
    }
}

// --- Mini App: «Отправить в чат» ----------------------------------------------

pub fn configure_api(cfg: &mut web::ServiceConfig) {
    cfg.service(send_file).service(send_exchange_file);
}

/// Sending to chat needs the bot's internal API; otherwise the button is hidden.
pub fn enabled(config: &AppConfig) -> bool {
    config.internal_api_token.is_some() && config.telegram.is_some()
}

fn queue_reply(
    config: &AppConfig,
    jobs: &ChatJobs,
    user: &ApiUser,
    name: &str,
    path: &Path,
) -> HttpResponse {
    if !enabled(config) {
        return reply::error(
            StatusCode::NOT_FOUND,
            "disabled",
            "Отправка в чат выключена: бот не подключён к файловому менеджеру",
        );
    }
    let size = match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => meta.len(),
        _ => return reply::storage(StorageError::NotFound),
    };
    match jobs.push(
        user.telegram_id,
        &user.username,
        name,
        size,
        path.to_path_buf(),
    ) {
        Ok(id) => {
            tracing::info!(user = %user.username, job = id, size, "file queued for the chat with the bot");
            reply::ok(json!({ "message": "Файл придёт в чат с ботом", "job": id }))
        }
        Err(QueueError::Full) => reply::error(
            StatusCode::TOO_MANY_REQUESTS,
            "queue_full",
            "Слишком много файлов ждут отправки — подождите, пока придут предыдущие",
        ),
    }
}

#[post("/files/{id}/send-to-chat")]
async fn send_file(
    id: web::Path<String>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    jobs: web::Data<ChatJobs>,
) -> HttpResponse {
    let (item, path) = match api::resolve(&config, &user, &id) {
        Ok(v) => v,
        Err(e) => return reply::storage(e),
    };
    let Some(name) = item.name else {
        return reply::bad_request("В чат можно отправить только файл");
    };
    queue_reply(&config, &jobs, &user, &name, &path)
}

#[post("/exchange/items/{id}/send-to-chat")]
async fn send_exchange_file(
    id: web::Path<String>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    jobs: web::Data<ChatJobs>,
) -> HttpResponse {
    match crate::exchange::api_file(&config, &store, &user, &id) {
        Ok((_, _, name, path)) => queue_reply(&config, &jobs, &user, &name, &path),
        Err(e) => reply::storage(e),
    }
}

// --- Internal API for the bot -------------------------------------------------

pub fn configure_internal(cfg: &mut web::ServiceConfig) {
    cfg.service(save_from_chat)
        .service(take_jobs)
        .service(job_content)
        .service(job_done);
}

#[derive(Deserialize)]
struct SaveQuery {
    telegram_id: i64,
    name: String,
    /// `my` (default) or `shared`.
    #[serde(default)]
    scope: Option<String>,
    /// Empty: by extension, as for uploads.
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    path: String,
}

fn zone_title(zone: Zone) -> &'static str {
    match zone {
        Zone::My => "Мои файлы",
        Zone::Shared => "Общие файлы",
    }
}

/// Save a file the account sent in the chat. Body: the raw file content.
#[post("/files")]
async fn save_from_chat(
    req: HttpRequest,
    query: web::Query<SaveQuery>,
    mut body: web::Payload,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    let Some(account) = store.account_by_telegram(query.telegram_id, &config.admin_username) else {
        return reply::error(
            StatusCode::NOT_FOUND,
            "not_linked",
            "Этот Telegram не привязан к аккаунту файлового менеджера",
        );
    };
    let prepared = (|| -> Result<(Zone, String, RelPath, String), StorageError> {
        let zone = Zone::parse(query.scope.as_deref().unwrap_or("my")).ok_or(StorageError::Zone)?;
        // Names from Telegram may carry anything: keep the safe part, never a path.
        let name = sanitize_file_name(&query.name).unwrap_or_else(|| "file".to_string());
        let category = api::upload_category(query.category.as_deref(), &name)?;
        let rel = RelPath::parse(&query.path).map_err(StorageError::Path)?;
        crate::storage::folder_dir(&config, zone, &account.username, &category, &rel)?;
        Ok((zone, category, rel, name))
    })();
    let (zone, category, rel, name) = match prepared {
        Ok(v) => v,
        Err(e) => return reply::storage(e),
    };
    let limit = config.max_file_size as u64;
    let too_large = || {
        reply::error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            format!("Файл больше лимита {}", format_bytes(limit)),
        )
    };
    let declared = req
        .headers()
        .get(actix_web::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|len| len > limit) {
        return too_large();
    }

    let staging = config.upload_dir.join(STAGING_DIR).join(&account.username);
    if let Err(e) = std::fs::create_dir_all(&staging) {
        return reply::storage(e.into());
    }
    // Removed on drop unless published: an interrupted transfer leaves nothing.
    let mut pending =
        match PendingUpload::create(&staging, &format!("chat-{}.part", rand::random::<u64>())) {
            Ok(p) => p,
            Err(e) => return reply::storage(e.into()),
        };
    let mut written: u64 = 0;
    while let Some(chunk) = body.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(_) => return reply::bad_request("Передача файла прервалась"),
        };
        written += chunk.len() as u64;
        if written > limit {
            return too_large();
        }
        let file = pending.file.as_mut().expect("open staged file");
        if let Err(e) = std::io::Write::write_all(file, &chunk) {
            return reply::storage(e.into());
        }
    }
    if let Some(file) = pending.file.as_mut() {
        if let Err(e) = std::io::Write::flush(file) {
            return reply::storage(e.into());
        }
    }
    pending.file.take();
    match api::finish_upload(
        &config,
        zone,
        &account.username,
        &category,
        &rel,
        &pending.path,
        &name,
    ) {
        Ok(item) => {
            // The staged name is gone now; nothing is left to clean up.
            let _ = pending.finish();
            tracing::info!(user = %account.username, scope = zone.as_str(), category = %category, size = written, "file saved from the chat with the bot");
            reply::ok(json!({
                "item": item,
                "message": format!("Сохранено в «{} › {}»", zone_title(zone), category),
            }))
        }
        Err(e) => reply::storage(e),
    }
}

#[derive(Deserialize)]
struct TakeQuery {
    #[serde(default)]
    wait: Option<u64>,
}

/// Jobs for the bot. With `wait`, holds the request up to that many seconds
/// (at most 30) until a job appears, so files arrive without polling delay.
#[get("/jobs")]
async fn take_jobs(query: web::Query<TakeQuery>, jobs: web::Data<ChatJobs>) -> HttpResponse {
    let deadline = Instant::now() + Duration::from_secs(query.wait.unwrap_or(0).min(MAX_WAIT_SECS));
    while !jobs.has_queued() && Instant::now() < deadline {
        actix_web::rt::time::sleep(POLL_STEP).await;
    }
    let list: Vec<_> = jobs
        .take(TAKE_LIMIT)
        .into_iter()
        .map(|j| {
            json!({
                "id": j.id,
                "telegram_id": j.telegram_id,
                "username": j.username,
                "name": j.name,
                "size": j.size,
            })
        })
        .collect();
    reply::ok(json!({ "jobs": list }))
}

fn job_not_found() -> HttpResponse {
    reply::error(
        StatusCode::NOT_FOUND,
        "no_job",
        "Задание не найдено или устарело",
    )
}

/// Content of a job the bot took; the file was checked when it was queued.
#[get("/jobs/{id}/content")]
async fn job_content(
    req: HttpRequest,
    id: web::Path<u64>,
    jobs: web::Data<ChatJobs>,
) -> HttpResponse {
    let Some(job) = jobs.leased(*id) else {
        return job_not_found();
    };
    // Renamed, deleted or replaced by a symlink since: not sent.
    match std::fs::symlink_metadata(&job.path) {
        Ok(meta) if meta.is_file() => {}
        _ => return reply::storage(StorageError::NotFound),
    }
    match NamedFile::open(&job.path) {
        Ok(file) => file
            .set_content_disposition(ContentDisposition {
                disposition: DispositionType::Attachment,
                parameters: vec![],
            })
            .into_response(&req),
        Err(e) => reply::storage(e.into()),
    }
}

#[derive(Deserialize)]
struct DoneBody {
    ok: bool,
}

#[post("/jobs/{id}/done")]
async fn job_done(
    id: web::Path<u64>,
    body: web::Json<DoneBody>,
    jobs: web::Data<ChatJobs>,
) -> HttpResponse {
    match jobs.finish(*id) {
        Some(job) => {
            if body.ok {
                tracing::info!(user = %job.username, job = job.id, "file sent to the chat with the bot");
            } else {
                tracing::warn!(user = %job.username, job = job.id, "bot could not send the file to the chat");
            }
            reply::ok(json!({}))
        }
        None => job_not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> PathBuf {
        PathBuf::from("/nonexistent/synthetic.txt")
    }

    #[test]
    fn queue_limits_per_user_and_lease() {
        let jobs = ChatJobs::default();
        for _ in 0..MAX_QUEUED_PER_USER {
            jobs.push(1, "anna", "a.txt", 1, path()).unwrap();
        }
        assert_eq!(
            jobs.push(1, "anna", "a.txt", 1, path()),
            Err(QueueError::Full)
        );
        jobs.push(2, "bob", "b.txt", 1, path()).unwrap();

        let taken = jobs.take(TAKE_LIMIT);
        assert_eq!(taken.len(), TAKE_LIMIT);
        assert!(taken.iter().all(|j| j.username == "anna"));
        assert!(jobs.leased(taken[0].id).is_some());
        assert!(jobs.finish(taken[0].id).is_some());
        assert!(jobs.leased(taken[0].id).is_none());

        // Taken jobs no longer count against the queue limit.
        jobs.push(1, "anna", "c.txt", 1, path()).unwrap();
        let rest = jobs.take(TAKE_LIMIT);
        assert_eq!(
            rest.iter().map(|j| j.username.as_str()).collect::<Vec<_>>(),
            ["bob", "anna"]
        );
        assert!(jobs.take(TAKE_LIMIT).is_empty());
    }
}
