//! Private doctor workspace bridge. Content stays in memory; the bot owns the
//! anonymization, feature permission and persistent database. No new listener.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use actix_web::http::StatusCode;
use actix_web::{get, post, web, HttpResponse};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{self, ApiUser};
use crate::config::AppConfig;
use crate::reply;
use crate::storage::StorageError;

const FILE_LIMIT: usize = 5 * 1024 * 1024;
const BODY_LIMIT: usize = 7 * 1024 * 1024;
const MEMORY_LIMIT: usize = 32 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(600);
const OPERATIONS: &[&str] = &[
    "status",
    "list",
    "create",
    "get",
    "review",
    "review_hide",
    "review_restore",
    "review_confirm",
    "review_cancel",
    "generate",
    "save",
    "archive",
    "export",
    "send_chat",
];

struct Job {
    telegram_id: i64,
    username: String,
    request_id: String,
    operation: String,
    payload: Option<Value>,
    result: Option<Value>,
    created: Instant,
    size: usize,
}

#[derive(Default)]
struct State {
    jobs: HashMap<String, Job>,
    last_poll: Option<Instant>,
}

impl State {
    fn purge(&mut self) {
        self.jobs.retain(|_, job| job.created.elapsed() < TTL);
    }
}

#[derive(Default)]
pub struct DoctorJobs {
    state: Mutex<State>,
}

fn bad(message: &str) -> HttpResponse {
    reply::bad_request(message)
}

impl DoctorJobs {
    fn enqueue(&self, user: &ApiUser, body: Command) -> HttpResponse {
        let mut state = self.state.lock().expect("doctor jobs");
        state.purge();
        // Idempotency is per authenticated account, never per client-supplied user.
        if let Some((id, _)) = state.jobs.iter().find(|(_, j)| {
            j.telegram_id == user.telegram_id
                && j.username == user.username
                && j.request_id == body.request_id
        }) {
            return reply::ok(json!({"job": id}));
        }
        if state
            .last_poll
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(60))
        {
            return reply::error(
                StatusCode::SERVICE_UNAVAILABLE,
                "doctor_offline",
                "Помощник врача пока не подключён. Попробуйте позже.",
            );
        }
        let size = body.payload.to_string().len();
        let mine = state
            .jobs
            .values()
            .filter(|j| j.telegram_id == user.telegram_id && j.result.is_none())
            .count();
        if mine >= 4
            || state.jobs.len() >= 128
            || state.jobs.values().map(|j| j.size).sum::<usize>() + size > MEMORY_LIMIT
        {
            return reply::error(
                StatusCode::TOO_MANY_REQUESTS,
                "doctor_busy",
                "Слишком много запросов. Дождитесь завершения предыдущего.",
            );
        }
        let id = format!("{:032x}", rand::random::<u128>());
        state.jobs.insert(
            id.clone(),
            Job {
                telegram_id: user.telegram_id,
                username: user.username.clone(),
                request_id: body.request_id,
                operation: body.operation,
                payload: Some(body.payload),
                result: None,
                created: Instant::now(),
                size,
            },
        );
        reply::ok(json!({"job": id}))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    request_id: String,
    operation: String,
    #[serde(default = "empty_object")]
    payload: Value,
}

fn empty_object() -> Value {
    json!({})
}

pub fn configure_api(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/doctor")
            .app_data(web::JsonConfig::default().limit(BODY_LIMIT))
            .service(submit)
            .service(poll),
    );
}

pub fn configure_internal(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/doctor")
            .app_data(web::JsonConfig::default().limit(BODY_LIMIT))
            .service(take)
            .service(finish),
    );
}

#[post("/jobs")]
async fn submit(
    user: web::ReqData<ApiUser>,
    body: web::Json<Command>,
    config: web::Data<AppConfig>,
    jobs: web::Data<DoctorJobs>,
) -> HttpResponse {
    if !crate::chat::enabled(&config) {
        return reply::error(
            StatusCode::NOT_FOUND,
            "disabled",
            "Помощник врача не подключён.",
        );
    }
    let mut body = body.into_inner();
    if !OPERATIONS.contains(&body.operation.as_str())
        || !body.payload.is_object()
        || !(8..=80).contains(&body.request_id.len())
        || !body
            .request_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return bad("Некорректное действие.");
    }
    // Selecting a server file uses exactly the regular file authorization and
    // path checks. Clients cannot name filesystem paths or another account.
    if let Some(id) = body.payload.get("source_id").and_then(Value::as_str) {
        if body.operation != "review" {
            return bad("Файл можно выбрать только для проверки.");
        }
        let (item, path) = match api::resolve(&config, &user, id) {
            Ok(value) => value,
            Err(error) => return reply::storage(error),
        };
        if !item
            .name
            .as_deref()
            .is_some_and(|name| name.to_lowercase().ends_with(".docx"))
        {
            return bad("Для помощника нужен документ .docx.");
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = match options.open(&path) {
            Ok(file) => file,
            Err(_) => return reply::storage(StorageError::NotFound),
        };
        let mut bytes = Vec::new();
        match file.metadata() {
            Ok(meta) if meta.is_file() && meta.len() <= FILE_LIMIT as u64 => (),
            _ => return bad("Нужен файл .docx размером до 5 МБ."),
        }
        if file
            .by_ref()
            .take(FILE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return bad("Не удалось прочитать файл.");
        }
        if bytes.len() > FILE_LIMIT {
            return bad("Документ больше 5 МБ.");
        }
        body.payload
            .as_object_mut()
            .expect("object")
            .remove("source_id");
        body.payload["content"] = json!(STANDARD.encode(bytes));
    }
    if let Some(content) = body.payload.get("content") {
        if body.operation != "review"
            || !content.is_string()
            || content.as_str().map_or(usize::MAX, str::len) > FILE_LIMIT.div_ceil(3) * 4
        {
            return bad("Нужен файл .docx размером до 5 МБ.");
        }
    }
    jobs.enqueue(&user, body)
}

#[get("/jobs/{id}")]
async fn poll(
    user: web::ReqData<ApiUser>,
    id: web::Path<String>,
    jobs: web::Data<DoctorJobs>,
) -> HttpResponse {
    let mut state = jobs.state.lock().expect("doctor jobs");
    state.purge();
    let Some(job) = state
        .jobs
        .get(id.as_str())
        .filter(|j| j.telegram_id == user.telegram_id && j.username == user.username)
    else {
        return reply::error(
            StatusCode::NOT_FOUND,
            "doctor_expired",
            "Запрос устарел. Откройте обращение заново, чтобы проверить результат.",
        );
    };
    let phase = if job.result.is_some() {
        "done"
    } else if job.payload.is_some() {
        "queued"
    } else {
        "running"
    };
    reply::ok(json!({"state": phase, "result": job.result}))
}

#[derive(Deserialize)]
struct Wait {
    #[serde(default)]
    wait: u64,
}

#[get("/jobs")]
async fn take(query: web::Query<Wait>, jobs: web::Data<DoctorJobs>) -> HttpResponse {
    let started = Instant::now();
    loop {
        {
            let mut state = jobs.state.lock().expect("doctor jobs");
            state.purge();
            state.last_poll = Some(Instant::now());
            // One lease per request: the bot caps its active workers at four.
            if let Some((id, job)) = state
                .jobs
                .iter_mut()
                .filter(|(_, j)| j.payload.is_some())
                .min_by_key(|(_, j)| j.created)
            {
                let payload = job.payload.take();
                job.size = 0;
                return reply::ok(json!({"jobs": [{"id": id, "telegram_id": job.telegram_id,
                    "operation": job.operation, "payload": payload}]}));
            }
        }
        if started.elapsed() >= Duration::from_secs(query.wait.min(25)) {
            return reply::ok(json!({"jobs": []}));
        }
        actix_web::rt::time::sleep(Duration::from_millis(150)).await;
    }
}

#[post("/jobs/{id}/done")]
async fn finish(
    id: web::Path<String>,
    result: web::Json<Value>,
    jobs: web::Data<DoctorJobs>,
) -> HttpResponse {
    let result = result.into_inner();
    if !result.get("ok").is_some_and(Value::is_boolean) {
        return bad("Некорректный результат.");
    }
    let mut state = jobs.state.lock().expect("doctor jobs");
    state.purge();
    let size = result.to_string().len();
    if size + state.jobs.values().map(|j| j.size).sum::<usize>() > MEMORY_LIMIT {
        return reply::error(
            StatusCode::TOO_MANY_REQUESTS,
            "doctor_busy",
            "Повторите передачу результата.",
        );
    }
    let Some(job) = state
        .jobs
        .get_mut(id.as_str())
        .filter(|j| j.payload.is_none())
    else {
        return reply::error(StatusCode::NOT_FOUND, "doctor_expired", "Запрос устарел.");
    };
    job.result = Some(result);
    job.size = size;
    reply::ok(json!({}))
}
