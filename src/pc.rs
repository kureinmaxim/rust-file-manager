//! «Загрузить с ПК»: a computer gets upload-only access to «Мои файлы» after
//! the owner approves it in the Mini App. This is the OAuth device flow
//! (RFC 8628):
//!
//! 1. `/pc` in the computer's browser calls `POST /api/v1/pc/pair` and gets a
//!    secret that only this page knows, plus a short code. The page shows the
//!    code as text and as a QR code `https://t.me/<bot>?startapp=pc_<code>`.
//! 2. The phone scans it with Telegram's scanner (or the camera, which opens
//!    the Mini App). The app shows which computer is asking and approves it:
//!    `POST /api/v1/computers/approve`.
//! 3. The page polls `POST /api/v1/pc/poll` with its secret and receives a
//!    `pc1.` token. Only the chunked upload API accepts it (see
//!    `api::require_bearer`).
//!
//! The code alone gives nothing: only the holder of the secret receives the
//! token. Pairings and sessions live in memory, so a restart disconnects every
//! computer. A session ends after 30 minutes without uploads, after 12 hours
//! at most, or when the phone disconnects it.
//!
//! The page also sends a random device id it keeps in localStorage. Unfinished
//! uploads belong to that id, not to the session, so after reconnecting (a
//! restart, a disconnect) the same browser resumes them; other computers and
//! the phone's own uploads stay invisible to it.

use std::collections::HashMap;
use std::future::{ready, Ready};
use std::sync::Mutex;

use actix_web::dev::Payload;
use actix_web::error::InternalError;
use actix_web::http::header;
use actix_web::http::StatusCode;
use actix_web::{delete, get, post, web, FromRequest, HttpMessage, HttpRequest, HttpResponse};
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::ApiUser;
use crate::config::AppConfig;
use crate::qr::QrCode;
use crate::ratelimit::RateLimiter;
use crate::reply::{self, now_secs};
use crate::tokens::{issue_pc, PcClaims, TokenKeys};
use crate::users::UserStore;

/// A code on the computer's screen stays valid this long.
pub const PAIR_TTL: u64 = 5 * 60;
/// How often the page asks whether the phone has approved it.
const POLL_INTERVAL: u64 = 2;
/// A session without uploads ends after this long.
pub const IDLE_TTL: u64 = 30 * 60;
/// And in any case after this long (also the token lifetime).
pub const SESSION_MAX: u64 = 12 * 60 * 60;
/// Codes waiting for approval, from all computers: a bound for memory.
const MAX_PAIRINGS: usize = 200;
const MAX_SESSIONS_PER_USER: usize = 5;
/// Crockford base32: no I, L, O, U, so the code reads and types unambiguously.
const CODE_ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// 8 symbols = 40 bits; with the approval rate limit a code cannot be guessed
/// within its 5 minutes.
const CODE_LEN: usize = 8;
const SECRET_LEN: usize = 32;
const SESSION_ID_LEN: usize = 16;

/// A computer that is allowed to upload, as the middleware found it.
#[derive(Clone, Debug)]
pub struct PcUser {
    pub user: ApiUser,
    pub session: String,
    /// The browser's device id: owner of its unfinished uploads.
    pub device: String,
}

/// The request was not made with a computer token.
fn not_a_computer() -> actix_web::Error {
    InternalError::from_response(
        "not a computer session",
        reply::error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Компьютер не подключён — отсканируйте код заново",
        ),
    )
    .into()
}

impl FromRequest for PcUser {
    type Error = actix_web::Error;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        ready(
            req.extensions()
                .get::<PcUser>()
                .cloned()
                .ok_or_else(not_a_computer),
        )
    }
}

#[derive(Clone, Debug)]
struct Approved {
    token: String,
    username: String,
    session: String,
}

struct Pairing {
    code: String,
    agent: String,
    device: String,
    expires: u64,
    approved: Option<Approved>,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub id: String,
    pub username: String,
    pub agent: String,
    pub device: String,
    pub created: u64,
    pub expires: u64,
    pub last_active: u64,
    pub files: u64,
    pub bytes: u64,
}

impl Session {
    fn alive(&self, now: u64) -> bool {
        now < self.expires && now < self.last_active + IDLE_TTL
    }

    /// When the session ends if nothing more is uploaded.
    fn ends_at(&self) -> u64 {
        self.expires.min(self.last_active + IDLE_TTL)
    }

    fn describe(&self) -> Value {
        json!({
            "id": self.id,
            "agent": self.agent,
            "created_at": self.created,
            "ends_at": self.ends_at(),
            "idle_timeout": IDLE_TTL,
            "files": self.files,
            "bytes": self.bytes,
        })
    }
}

#[derive(Default)]
struct State {
    /// Keyed by the page's secret.
    pairings: HashMap<String, Pairing>,
    /// Keyed by session id.
    sessions: HashMap<String, Session>,
}

impl State {
    fn prune(&mut self, now: u64) {
        self.pairings.retain(|_, p| now < p.expires);
        self.sessions.retain(|_, s| s.alive(now));
    }

    fn pending_by_code(&mut self, code: &str) -> Option<&mut Pairing> {
        self.pairings
            .values_mut()
            .find(|p| p.approved.is_none() && p.code == code)
    }
}

/// Pairings, sessions and their rate limits (shared by all workers).
pub struct Computers {
    state: Mutex<State>,
    /// New codes per client address: best effort, X-Forwarded-For can be
    /// forged. MAX_PAIRINGS bounds memory whatever the address.
    pair_limiter: RateLimiter,
    /// Code lookups and approvals per Telegram account.
    approve_limiter: RateLimiter,
}

impl Default for Computers {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
            pair_limiter: RateLimiter::new(20, 10 * 60),
            approve_limiter: RateLimiter::new(10, 10 * 60),
        }
    }
}

pub enum Poll {
    Pending {
        expires_in: u64,
    },
    Approved {
        token: String,
        username: String,
        session: Session,
    },
    Expired,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ApproveError {
    NotFound,
    TooMany,
}

fn random_string(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn random_code() -> String {
    let mut rng = rand::thread_rng();
    (0..CODE_LEN)
        .map(|_| char::from(CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())]))
        .collect()
}

/// The code from what the phone scanned or the user typed: a `startapp=pc_…`
/// link, `pc_…`, or the code itself with any spacing, dashes and case.
/// Letters that look like digits are read as those digits.
pub fn normalize_code(input: &str) -> Option<String> {
    let raw = match input.find("pc_") {
        Some(at) => &input[at + 3..],
        None => input,
    };
    let code: String = raw
        .chars()
        .take_while(|c| !matches!(c, '&' | '#' | '/'))
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect();
    (code.len() == CODE_LEN && code.bytes().all(|b| CODE_ALPHABET.contains(&b))).then_some(code)
}

/// The page's device id if it looks like one (letters and digits, 16–64),
/// otherwise a fresh random one: uploads then resume only within the session.
pub fn device_id(raw: &str) -> String {
    let valid = (16..=64).contains(&raw.len()) && raw.chars().all(|c| c.is_ascii_alphanumeric());
    if valid {
        raw.to_string()
    } else {
        random_string(SECRET_LEN)
    }
}

/// `ABCD-EFGH` for reading aloud and comparing with the phone.
pub fn display_code(code: &str) -> String {
    let (a, b) = code.split_at(code.len() / 2);
    format!("{a}-{b}")
}

/// "Chrome · Windows" from a User-Agent, built only from fixed names so the
/// phone never shows text a stranger chose.
pub fn describe_agent(user_agent: &str) -> String {
    let ua = user_agent.to_ascii_lowercase();
    let browser = [
        ("yabrowser", "Яндекс Браузер"),
        ("edg/", "Edge"),
        ("opr/", "Opera"),
        ("firefox/", "Firefox"),
        ("chrome/", "Chrome"),
        ("safari/", "Safari"),
    ]
    .iter()
    .find(|(needle, _)| ua.contains(needle))
    .map(|(_, name)| *name);
    let os = [
        ("windows", "Windows"),
        ("iphone", "iPhone"),
        ("ipad", "iPad"),
        ("android", "Android"),
        ("cros", "ChromeOS"),
        ("mac os", "macOS"),
        ("linux", "Linux"),
    ]
    .iter()
    .find(|(needle, _)| ua.contains(needle))
    .map(|(_, name)| *name);
    match (browser, os) {
        (Some(b), Some(o)) => format!("{b} · {o}"),
        (Some(b), None) => b.to_string(),
        (None, Some(o)) => format!("Браузер · {o}"),
        (None, None) => "Браузер".to_string(),
    }
}

impl Computers {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("computers lock")
    }

    /// A new code for a computer: `(secret, code)`, or `None` when too many
    /// codes are waiting.
    pub fn pair(&self, agent: String, device: String, now: u64) -> Option<(String, String)> {
        let mut state = self.lock();
        state.prune(now);
        if state.pairings.len() >= MAX_PAIRINGS {
            return None;
        }
        let code = loop {
            let code = random_code();
            if !state.pairings.values().any(|p| p.code == code) {
                break code;
            }
        };
        let secret = random_string(SECRET_LEN);
        state.pairings.insert(
            secret.clone(),
            Pairing {
                code: code.clone(),
                agent,
                device,
                expires: now + PAIR_TTL,
                approved: None,
            },
        );
        Some((secret, code))
    }

    /// The page's question «approved yet?». The token is handed out once.
    pub fn poll(&self, secret: &str, now: u64) -> Poll {
        let mut state = self.lock();
        state.prune(now);
        let Some(pairing) = state.pairings.get(secret) else {
            return Poll::Expired;
        };
        let Some(approved) = pairing.approved.clone() else {
            return Poll::Pending {
                expires_in: pairing.expires - now,
            };
        };
        state.pairings.remove(secret);
        match state.sessions.get(&approved.session) {
            Some(session) => Poll::Approved {
                token: approved.token,
                username: approved.username,
                session: session.clone(),
            },
            // Disconnected from the phone before the page picked the token up.
            None => Poll::Expired,
        }
    }

    /// Which computer waits under this code (for the phone's confirmation).
    pub fn lookup(&self, code: &str, now: u64) -> Option<(String, u64)> {
        let mut state = self.lock();
        state.prune(now);
        state
            .pending_by_code(code)
            .map(|p| (p.agent.clone(), p.expires - now))
    }

    /// Approve a waiting code for `user`: a session and its token.
    pub fn approve(
        &self,
        keys: &TokenKeys,
        user: &ApiUser,
        token_version: u32,
        code: &str,
        now: u64,
    ) -> Result<Session, ApproveError> {
        let mut state = self.lock();
        state.prune(now);
        let open = state
            .sessions
            .values()
            .filter(|s| s.username == user.username)
            .count();
        if state.pending_by_code(code).is_none() {
            return Err(ApproveError::NotFound);
        }
        if open >= MAX_SESSIONS_PER_USER {
            return Err(ApproveError::TooMany);
        }
        let id = random_string(SESSION_ID_LEN);
        let token = issue_pc(
            keys,
            &PcClaims {
                u: user.username.clone(),
                tv: token_version,
                tg: user.telegram_id,
                sid: id.clone(),
                iat: now,
                exp: now + SESSION_MAX,
            },
        );
        let pairing = state.pending_by_code(code).expect("checked above");
        pairing.approved = Some(Approved {
            token,
            username: user.username.clone(),
            session: id.clone(),
        });
        let session = Session {
            id: id.clone(),
            username: user.username.clone(),
            agent: pairing.agent.clone(),
            device: pairing.device.clone(),
            created: now,
            expires: now + SESSION_MAX,
            last_active: now,
            files: 0,
            bytes: 0,
        };
        state.sessions.insert(id, session.clone());
        Ok(session)
    }

    /// The device id of a computer token's session, if it is still
    /// connected for `username`.
    pub fn device(&self, id: &str, username: &str, now: u64) -> Option<String> {
        self.lock()
            .sessions
            .get(id)
            .filter(|s| s.username == username && s.alive(now))
            .map(|s| s.device.clone())
    }

    /// An upload request keeps the session from ending as idle.
    pub fn touch(&self, id: &str, now: u64) {
        if let Some(s) = self.lock().sessions.get_mut(id) {
            s.last_active = s.last_active.max(now);
        }
    }

    pub fn record_upload(&self, id: &str, bytes: u64) {
        if let Some(s) = self.lock().sessions.get_mut(id) {
            s.files += 1;
            s.bytes += bytes;
        }
    }

    pub fn session(&self, id: &str, now: u64) -> Option<Session> {
        let mut state = self.lock();
        state.prune(now);
        state.sessions.get(id).cloned()
    }

    pub fn list(&self, username: &str, now: u64) -> Vec<Session> {
        let mut state = self.lock();
        state.prune(now);
        let mut list: Vec<Session> = state
            .sessions
            .values()
            .filter(|s| s.username == username)
            .cloned()
            .collect();
        list.sort_by_key(|s| s.created);
        list
    }

    pub fn revoke(&self, username: &str, id: &str) -> bool {
        let mut state = self.lock();
        let owned = state
            .sessions
            .get(id)
            .is_some_and(|s| s.username == username);
        if owned {
            state.sessions.remove(id);
        }
        owned
    }
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

/// Public routes of the computer's page (no token yet).
pub fn configure_public(cfg: &mut web::ServiceConfig) {
    cfg.service(pair).service(poll);
}

/// Routes behind `require_bearer`: the Mini App manages computers, a
/// computer reads and ends its own session.
pub fn configure_api(cfg: &mut web::ServiceConfig) {
    cfg.service(list_computers)
        .service(lookup_code)
        .service(approve_code)
        .service(disconnect)
        .service(own_session)
        .service(end_own_session);
}

fn unavailable() -> HttpResponse {
    reply::error(
        StatusCode::NOT_FOUND,
        "disabled",
        "Загрузка с компьютера недоступна: мини-приложение выключено",
    )
}

fn expired() -> HttpResponse {
    reply::error(
        StatusCode::GONE,
        "pair_expired",
        "Код устарел — получите новый",
    )
}

fn code_not_found() -> HttpResponse {
    reply::error(
        StatusCode::NOT_FOUND,
        "pair_not_found",
        "Код не найден или устарел. Обновите страницу на компьютере и отсканируйте новый код.",
    )
}

fn rate_limited() -> HttpResponse {
    reply::error(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        "Слишком много попыток. Подождите 10 минут.",
    )
}

/// Free space and limits for the upload screens.
pub fn capacity(config: &AppConfig) -> Value {
    json!({
        "available_bytes": crate::uploads::available_bytes(config),
        "max_file_size": config.max_chunked_file_size,
        "chunk_size": config.upload_chunk_size,
    })
}

#[derive(Deserialize)]
struct PairBody {
    #[serde(default)]
    device: String,
}

#[post("/pc/pair")]
async fn pair(
    req: HttpRequest,
    body: web::Json<PairBody>,
    config: web::Data<AppConfig>,
    computers: web::Data<Computers>,
) -> HttpResponse {
    let Some(tg) = config.telegram.as_ref() else {
        return unavailable();
    };
    let now = now_secs();
    let client = req
        .connection_info()
        .realip_remote_addr()
        .unwrap_or("unknown")
        .to_string();
    if !computers.pair_limiter.allow(&format!("pair:{client}"), now) {
        return reply::error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Слишком много новых кодов. Подождите несколько минут.",
        );
    }
    let agent = describe_agent(
        req.headers()
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
    );
    let Some((secret, code)) = computers.pair(agent, device_id(&body.device), now) else {
        return reply::error(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "Сейчас слишком много подключений. Попробуйте через минуту.",
        );
    };
    let link = format!("https://t.me/{}?startapp=pc_{code}", tg.bot_username);
    let qr =
        QrCode::encode(link.as_bytes()).map(|q| json!({ "size": q.size(), "path": q.svg_path() }));
    reply::ok(json!({
        "secret": secret,
        "code": display_code(&code),
        "link": link,
        "qr": qr,
        "expires_in": PAIR_TTL,
        "interval": POLL_INTERVAL,
    }))
}

#[derive(Deserialize)]
struct PollBody {
    secret: String,
}

#[post("/pc/poll")]
async fn poll(
    body: web::Json<PollBody>,
    config: web::Data<AppConfig>,
    computers: web::Data<Computers>,
) -> HttpResponse {
    match computers.poll(&body.secret, now_secs()) {
        Poll::Pending { expires_in } => {
            reply::ok(json!({ "status": "pending", "expires_in": expires_in }))
        }
        Poll::Approved {
            token,
            username,
            session,
        } => reply::ok(json!({
            "status": "approved",
            "access_token": token,
            "user": { "username": username },
            "computer": session.describe(),
            "capacity": capacity(&config),
            "limits": {
                "max_file_size": config.max_chunked_file_size,
                "chunk_size": config.upload_chunk_size,
            },
        })),
        Poll::Expired => expired(),
    }
}

#[get("/computers")]
async fn list_computers(
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    computers: web::Data<Computers>,
) -> HttpResponse {
    let list: Vec<Value> = computers
        .list(&user.username, now_secs())
        .iter()
        .map(Session::describe)
        .collect();
    reply::ok(json!({
        "computers": list,
        "pc_url": config.public_base_url.as_ref().map(|base| format!("{base}/pc")),
    }))
}

#[derive(Deserialize)]
struct CodeBody {
    code: String,
}

/// The code from the request, after the per-account attempt limit.
fn checked_code(
    computers: &Computers,
    user: &ApiUser,
    raw: &str,
    now: u64,
) -> Result<String, HttpResponse> {
    if !computers
        .approve_limiter
        .allow(&format!("tg:{}", user.telegram_id), now)
    {
        return Err(rate_limited());
    }
    normalize_code(raw).ok_or_else(|| {
        reply::error(
            StatusCode::BAD_REQUEST,
            "bad_code",
            "Это не код компьютера. Он выглядит как ABCD-2345 и есть на странице /pc.",
        )
    })
}

#[post("/computers/lookup")]
async fn lookup_code(
    body: web::Json<CodeBody>,
    user: web::ReqData<ApiUser>,
    computers: web::Data<Computers>,
) -> HttpResponse {
    let now = now_secs();
    let code = match checked_code(&computers, &user, &body.code, now) {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match computers.lookup(&code, now) {
        Some((agent, expires_in)) => reply::ok(json!({
            "code": display_code(&code),
            "agent": agent,
            "expires_in": expires_in,
        })),
        None => code_not_found(),
    }
}

#[post("/computers/approve")]
async fn approve_code(
    body: web::Json<CodeBody>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    keys: web::Data<TokenKeys>,
    computers: web::Data<Computers>,
) -> HttpResponse {
    let now = now_secs();
    let code = match checked_code(&computers, &user, &body.code, now) {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let Some(account) = store.account(&user.username, &config.admin_username) else {
        return code_not_found();
    };
    match computers.approve(&keys, &user, account.token_version, &code, now) {
        Ok(session) => {
            tracing::info!(user = %user.username, agent = %session.agent, "computer connected for uploads");
            reply::ok(json!({
                "computer": session.describe(),
                "message": "Компьютер подключён — можно загружать файлы",
            }))
        }
        Err(ApproveError::NotFound) => code_not_found(),
        Err(ApproveError::TooMany) => reply::error(
            StatusCode::CONFLICT,
            "too_many_computers",
            format!(
                "Уже подключено {MAX_SESSIONS_PER_USER} компьютеров. Отключите ненужные и повторите."
            ),
        ),
    }
}

#[delete("/computers/{id}")]
async fn disconnect(
    id: web::Path<String>,
    user: web::ReqData<ApiUser>,
    computers: web::Data<Computers>,
) -> HttpResponse {
    if computers.revoke(&user.username, &id) {
        tracing::info!(user = %user.username, "computer disconnected from the Mini App");
        reply::ok(json!({ "message": "Компьютер отключён" }))
    } else {
        reply::error(
            StatusCode::NOT_FOUND,
            "not_found",
            "Этот компьютер уже отключён",
        )
    }
}

/// The computer's own view: who it uploads for, until when, how much fits.
#[get("/pc/session")]
async fn own_session(
    pc: PcUser,
    config: web::Data<AppConfig>,
    computers: web::Data<Computers>,
) -> HttpResponse {
    match computers.session(&pc.session, now_secs()) {
        Some(session) => reply::ok(json!({
            "user": { "username": pc.user.username },
            "computer": session.describe(),
            "capacity": capacity(&config),
        })),
        None => reply::error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Компьютер отключён — отсканируйте код заново",
        ),
    }
}

#[delete("/pc/session")]
async fn end_own_session(pc: PcUser, computers: web::Data<Computers>) -> HttpResponse {
    computers.revoke(&pc.user.username, &pc.session);
    tracing::info!(user = %pc.user.username, "computer disconnected itself");
    reply::ok(json!({ "message": "Компьютер отключён" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anna() -> ApiUser {
        ApiUser {
            username: "anna".into(),
            is_admin: false,
            telegram_id: 101,
        }
    }

    fn keys() -> TokenKeys {
        TokenKeys::derive(&[7u8; 64])
    }

    #[test]
    fn codes_are_normalized_from_links_and_typing() {
        assert_eq!(normalize_code("ABCD2345").as_deref(), Some("ABCD2345"));
        assert_eq!(normalize_code(" abcd-2345 ").as_deref(), Some("ABCD2345"));
        // O → 0, I/L → 1: what people type for the digits.
        assert_eq!(normalize_code("ABCD-O1IL").as_deref(), Some("ABCD0111"));
        assert_eq!(
            normalize_code("https://t.me/files_bot?startapp=pc_ABCD2345").as_deref(),
            Some("ABCD2345")
        );
        assert_eq!(
            normalize_code("https://t.me/files_bot?startapp=pc_ABCD2345&x=1").as_deref(),
            Some("ABCD2345")
        );
        assert_eq!(normalize_code("pc_abcd2345").as_deref(), Some("ABCD2345"));
        assert_eq!(normalize_code("ABCD234"), None);
        assert_eq!(normalize_code("ABCD23456"), None);
        assert_eq!(normalize_code("ABCD-23U5"), None); // U is not in the alphabet
        assert_eq!(
            normalize_code("https://t.me/files_bot?startapp=inv_x"),
            None
        );
        assert_eq!(display_code("ABCD2345"), "ABCD-2345");
    }

    #[test]
    fn device_ids_are_kept_only_when_plausible() {
        let id = "Ab3".repeat(8);
        assert_eq!(device_id(&id), id);
        for bad in ["", "short", "has space in the middle!", &"x".repeat(65)] {
            let made = device_id(bad);
            assert_ne!(made, bad);
            assert_eq!(made.len(), SECRET_LEN);
        }
    }

    #[test]
    fn agents_are_described_with_fixed_names() {
        let chrome_win = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
        assert_eq!(describe_agent(chrome_win), "Chrome · Windows");
        let edge = format!("{chrome_win} Edg/130.0.0.0");
        assert_eq!(describe_agent(&edge), "Edge · Windows");
        let safari = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
        assert_eq!(describe_agent(safari), "Safari · macOS");
        assert_eq!(
            describe_agent(
                "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0"
            ),
            "Firefox · Linux"
        );
        assert_eq!(describe_agent("<script>alert(1)</script>"), "Браузер");
        assert_eq!(describe_agent(""), "Браузер");
    }

    #[test]
    fn pairing_approval_and_single_token_handout() {
        let computers = Computers::default();
        let (secret, code) = computers
            .pair("Chrome · Windows".into(), "d".repeat(32), 1000)
            .unwrap();
        assert_eq!(code.len(), CODE_LEN);
        assert!(matches!(
            computers.poll(&secret, 1001),
            Poll::Pending { expires_in: 299 }
        ));
        assert_eq!(
            computers.lookup(&code, 1002),
            Some(("Chrome · Windows".to_string(), 298))
        );
        let session = computers.approve(&keys(), &anna(), 3, &code, 1003).unwrap();
        // The code is used up: a second approval or lookup finds nothing.
        assert_eq!(
            computers
                .approve(&keys(), &anna(), 3, &code, 1004)
                .unwrap_err(),
            ApproveError::NotFound
        );
        assert_eq!(computers.lookup(&code, 1004), None);
        let Poll::Approved {
            token, username, ..
        } = computers.poll(&secret, 1005)
        else {
            panic!("approved");
        };
        assert_eq!(username, "anna");
        let claims = crate::tokens::verify_pc(&keys(), &token, 1006).unwrap();
        assert_eq!(claims.sid, session.id);
        assert_eq!(claims.tv, 3);
        assert_eq!(claims.tg, 101);
        // Handed out once.
        assert!(matches!(computers.poll(&secret, 1006), Poll::Expired));
        assert!(matches!(computers.poll("unknown", 1006), Poll::Expired));
        assert_eq!(
            computers.device(&session.id, "anna", 1007),
            Some("d".repeat(32))
        );
        assert_eq!(computers.device(&session.id, "bob", 1007), None);
    }

    #[test]
    fn codes_expire_and_sessions_end_when_idle_or_revoked() {
        let computers = Computers::default();
        let (secret, code) = computers
            .pair("Браузер".into(), "d".repeat(32), 1000)
            .unwrap();
        assert!(matches!(
            computers.poll(&secret, 1000 + PAIR_TTL),
            Poll::Expired
        ));
        assert_eq!(
            computers
                .approve(&keys(), &anna(), 0, &code, 1000 + PAIR_TTL)
                .unwrap_err(),
            ApproveError::NotFound
        );

        let (_, code) = computers
            .pair("Браузер".into(), "d".repeat(32), 2000)
            .unwrap();
        let s = computers.approve(&keys(), &anna(), 0, &code, 2000).unwrap();
        assert!(computers
            .device(&s.id, "anna", 2000 + IDLE_TTL - 1)
            .is_some());
        computers.touch(&s.id, 2000 + IDLE_TTL - 1);
        assert!(computers
            .device(&s.id, "anna", 2000 + 2 * IDLE_TTL - 2)
            .is_some());
        assert!(computers
            .device(&s.id, "anna", 2000 + 2 * IDLE_TTL - 1)
            .is_none());

        // Uploading every few minutes does not extend past SESSION_MAX.
        let (_, code) = computers
            .pair("Браузер".into(), "d".repeat(32), 10_000)
            .unwrap();
        let s = computers
            .approve(&keys(), &anna(), 0, &code, 10_000)
            .unwrap();
        let mut t = 10_000;
        while t < 10_000 + SESSION_MAX {
            computers.touch(&s.id, t);
            t += 600;
        }
        assert!(computers
            .device(&s.id, "anna", 10_000 + SESSION_MAX)
            .is_none());

        let (_, code) = computers
            .pair("Браузер".into(), "d".repeat(32), 50_000)
            .unwrap();
        let s = computers
            .approve(&keys(), &anna(), 0, &code, 50_000)
            .unwrap();
        assert!(!computers.revoke("bob", &s.id));
        assert!(computers.revoke("anna", &s.id));
        assert!(computers.device(&s.id, "anna", 50_001).is_none());
    }

    #[test]
    fn limits_on_waiting_codes_and_sessions() {
        let computers = Computers::default();
        for i in 0..MAX_PAIRINGS {
            assert!(computers
                .pair(format!("{i}"), "d".repeat(32), 1000)
                .is_some());
        }
        assert!(computers
            .pair("one more".into(), "d".repeat(32), 1000)
            .is_none());
        // Expired codes free their places.
        assert!(computers
            .pair("later".into(), "d".repeat(32), 1000 + PAIR_TTL)
            .is_some());

        let computers = Computers::default();
        for _ in 0..MAX_SESSIONS_PER_USER {
            let (_, code) = computers
                .pair("Браузер".into(), "d".repeat(32), 1000)
                .unwrap();
            computers.approve(&keys(), &anna(), 0, &code, 1000).unwrap();
        }
        let (_, code) = computers
            .pair("Браузер".into(), "d".repeat(32), 1000)
            .unwrap();
        assert_eq!(
            computers
                .approve(&keys(), &anna(), 0, &code, 1000)
                .unwrap_err(),
            ApproveError::TooMany
        );
        assert_eq!(computers.list("anna", 1000).len(), MAX_SESSIONS_PER_USER);
        assert!(computers.list("bob", 1000).is_empty());
    }
}
