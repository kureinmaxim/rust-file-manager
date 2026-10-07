//! Mini App API: `/api/v1/*` with Bearer access tokens, and `/d/<token>`
//! signed downloads (TELEGRAM_MINIAPP_PLAN.md §7).
//!
//! The authenticated username always comes from a verified token (itself
//! issued from Telegram-signed initData); request parameters only ever select
//! a zone, category, folder and file *inside* that user's reach, through the
//! same storage functions the web UI uses.

use std::fs;
use std::path::Path;

use actix_files::NamedFile;
use actix_web::body::{BoxBody, MessageBody};
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::http::header::{
    self, Charset, ContentDisposition, DispositionParam, DispositionType, ExtendedValue,
};
use actix_web::http::StatusCode;
use actix_web::middleware::{from_fn, Next};
use actix_web::{delete, get, post, route, web, Error, HttpMessage, HttpRequest, HttpResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::categories::{category_for_extension, category_rel_dir};
use crate::config::AppConfig;
use crate::paths::RelPath;
use crate::ratelimit::RateLimiter;
use crate::reply::{self, now_secs};
use crate::storage;
use crate::storage::{
    category_dir, category_listing, decode_id, dir_stats, disk_usage, encode_id, exact_file_name,
    file_path, folder_dir, publish_file, read_dir_entries, walk, DirEntry, ItemRef, StorageError,
    Zone, WALK_LIMIT,
};
use crate::tg_auth::{public_key, verify_init_data, InitData};
use crate::tokens::{
    issue_access, sign_url, verify_access, verify_url, AccessClaims, Disposition, TokenKeys,
    UrlClaims, ACCESS_TTL_SECS,
};
use crate::uploads;
use crate::users::{validate_username, Account, LinkError, UserStore};

/// Telegram user authenticated by an access token.
#[derive(Clone, Debug)]
pub struct ApiUser {
    pub username: String,
    pub is_admin: bool,
    pub telegram_id: i64,
}

/// Password attempts from the Mini App: 5 per Telegram account per 15 minutes.
pub fn link_limiter() -> RateLimiter {
    RateLimiter::new(5, 15 * 60)
}

const LIST_LIMIT: usize = 500;
const SEARCH_LIMIT: usize = 200;
const RECENT_LIMIT: usize = 20;
const DOWNLOAD_URL_TTL: u64 = 5 * 60;
const STREAM_URL_TTL: u64 = 6 * 60 * 60;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(signed_download).service(
        web::scope("/api/v1")
            .service(tg_session)
            .service(tg_link)
            .service(tg_register)
            .service(
                web::scope("")
                    .wrap(from_fn(require_bearer))
                    .service(me)
                    .service(tg_unlink)
                    .service(revoke_all)
                    .service(overview)
                    .service(list_files)
                    .service(create_folder)
                    .service(item_details)
                    .service(rename_item)
                    .service(delete_item)
                    .service(make_link)
                    .configure(uploads::configure),
            ),
    );
}

fn unauthorized() -> HttpResponse {
    reply::error(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "Сеанс истёк — закройте и откройте приложение заново",
    )
}

async fn require_bearer(
    req: ServiceRequest,
    next: Next<impl MessageBody + 'static>,
) -> Result<ServiceResponse<BoxBody>, Error> {
    let user = (|| {
        let config = req.app_data::<web::Data<AppConfig>>()?;
        let store = req.app_data::<web::Data<UserStore>>()?;
        let keys = req.app_data::<web::Data<TokenKeys>>()?;
        let token = req
            .headers()
            .get(header::AUTHORIZATION)?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")?;
        let claims = verify_access(keys, token, now_secs())?;
        // Deleted users and revoked tokens (token_version bump) stop here.
        let account = store
            .account(&claims.u, &config.admin_username)
            .filter(|a| a.token_version == claims.tv && a.telegram_id == Some(claims.tg))?;
        Some(ApiUser {
            username: account.username,
            is_admin: account.is_admin,
            telegram_id: claims.tg,
        })
    })();
    match user {
        Some(user) => {
            req.extensions_mut().insert(user);
            Ok(next.call(req).await?.map_into_boxed_body())
        }
        None => {
            let (req, _) = req.into_parts();
            Ok(ServiceResponse::new(req, unauthorized()))
        }
    }
}

fn session_reply(
    config: &AppConfig,
    keys: &TokenKeys,
    account: &Account,
    telegram_id: i64,
) -> HttpResponse {
    let now = now_secs();
    let token = issue_access(
        keys,
        &AccessClaims {
            u: account.username.clone(),
            tv: account.token_version,
            tg: telegram_id,
            iat: now,
            exp: now + ACCESS_TTL_SECS,
        },
    );
    reply::ok(json!({
        "access_token": token,
        "expires_in": ACCESS_TTL_SECS,
        "user": {
            "username": account.username,
            "is_admin": account.is_admin,
            "telegram_id": telegram_id,
        },
        "limits": limits(config),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

fn limits(config: &AppConfig) -> serde_json::Value {
    json!({
        "max_file_size": config.max_chunked_file_size,
        "chunk_size": config.upload_chunk_size,
    })
}

/// Verify initData from a request body, or the error reply to send.
fn verified_init_data(config: &AppConfig, raw: &str) -> Result<InitData, HttpResponse> {
    let Some(tg) = config.telegram.as_ref() else {
        return Err(reply::error(
            StatusCode::NOT_FOUND,
            "disabled",
            "Мини-приложение выключено",
        ));
    };
    verify_init_data(
        raw,
        tg.bot_id,
        &public_key(tg),
        now_secs(),
        tg.init_data_max_age,
    )
    .map_err(|e| {
        tracing::warn!(reason = ?e, "rejected Mini App initData");
        reply::error(StatusCode::UNAUTHORIZED, "bad_init_data", e.message())
    })
}

fn invite_from(start_param: Option<&str>) -> Option<&str> {
    start_param
        .and_then(|p| p.strip_prefix("inv_"))
        .filter(|t| !t.is_empty())
}

#[derive(Deserialize)]
struct SessionBody {
    init_data: String,
}

#[post("/tg/session")]
async fn tg_session(
    body: web::Json<SessionBody>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    keys: web::Data<TokenKeys>,
) -> HttpResponse {
    let data = match verified_init_data(&config, &body.init_data) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    if let Some(account) = store.account_by_telegram(data.user.id, &config.admin_username) {
        return session_reply(&config, &keys, &account, data.user.id);
    }
    // Not linked yet: tell the app enough to show the link/register screen.
    let suggested = data
        .user
        .username
        .as_deref()
        .and_then(|u| validate_username(u).ok())
        .filter(|u| *u != config.admin_username && !store.user_exists(u))
        .unwrap_or_else(|| format!("tg{}", data.user.id));
    let invite_valid =
        invite_from(data.start_param.as_deref()).is_some_and(|t| store.invite_valid(t));
    reply::error_with(
        StatusCode::FORBIDDEN,
        json!({
            "code": "not_linked",
            "message": "Telegram ещё не привязан к аккаунту файлового менеджера",
            "telegram": {
                "id": data.user.id,
                "first_name": data.user.first_name,
                "username": data.user.username,
            },
            "suggested_username": suggested,
            "invite_valid": invite_valid,
        }),
    )
}

#[derive(Deserialize)]
struct LinkBody {
    init_data: String,
    username: String,
    password: String,
}

#[post("/tg/link")]
async fn tg_link(
    body: web::Json<LinkBody>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    keys: web::Data<TokenKeys>,
    limiter: web::Data<RateLimiter>,
) -> HttpResponse {
    let data = match verified_init_data(&config, &body.init_data) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    if !limiter.allow(&format!("tg:{}", data.user.id), now_secs()) {
        return reply::error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Слишком много попыток. Подождите 15 минут.",
        );
    }
    let username = body.username.trim().to_lowercase();
    let password = body.password.clone();
    let (cfg, st, name) = (config.clone(), store.clone(), username.clone());
    // bcrypt is deliberately slow: keep it off the async workers.
    let valid = web::block(move || {
        if name == cfg.admin_username {
            bcrypt::verify(&password, &cfg.admin_password_hash).unwrap_or(false)
        } else {
            st.verify_password(&name, &password)
        }
    })
    .await
    .unwrap_or(false);
    if !valid {
        tracing::warn!(telegram_id = data.user.id, "failed Mini App link attempt");
        return reply::error(
            StatusCode::UNAUTHORIZED,
            "bad_credentials",
            "Неверное имя пользователя или пароль",
        );
    }
    match store.link_telegram(&username, &config.admin_username, data.user.id) {
        Ok(()) => {}
        Err(LinkError::AlreadyLinked) => return reply::error(
            StatusCode::CONFLICT,
            "already_linked",
            "Этот аккаунт уже привязан к другому Telegram. Попросите администратора отвязать его.",
        ),
        Err(LinkError::Save(e)) => {
            return reply::error(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e)
        }
        Err(_) => {
            return reply::error(
                StatusCode::UNAUTHORIZED,
                "bad_credentials",
                "Неверное имя пользователя или пароль",
            )
        }
    }
    tracing::info!(user = %username, telegram_id = data.user.id, "telegram linked from Mini App");
    match store.account(&username, &config.admin_username) {
        Some(account) => session_reply(&config, &keys, &account, data.user.id),
        None => unauthorized(),
    }
}

#[derive(Deserialize)]
struct RegisterBody {
    init_data: String,
    username: String,
    #[serde(default)]
    invite_token: Option<String>,
}

#[post("/tg/register")]
async fn tg_register(
    body: web::Json<RegisterBody>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    keys: web::Data<TokenKeys>,
) -> HttpResponse {
    let data = match verified_init_data(&config, &body.init_data) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    if store
        .account_by_telegram(data.user.id, &config.admin_username)
        .is_some()
    {
        return reply::error(
            StatusCode::CONFLICT,
            "already_linked",
            "Этот Telegram уже привязан к аккаунту",
        );
    }
    let username = match validate_username(&body.username) {
        Ok(u) => u,
        Err(e) => return reply::bad_request(e),
    };
    if username == config.admin_username || store.user_exists(&username) {
        return reply::error(
            StatusCode::CONFLICT,
            "username_taken",
            "Пользователь с таким именем уже существует",
        );
    }
    let token = body
        .invite_token
        .clone()
        .filter(|t| !t.is_empty())
        .or_else(|| invite_from(data.start_param.as_deref()).map(str::to_string));
    let Some(token) = token else {
        return reply::error(
            StatusCode::FORBIDDEN,
            "no_invite",
            "Нужна ссылка-приглашение от администратора",
        );
    };
    // Consume the invite first: each link registers exactly one account.
    if let Err(e) = store.take_invite(&token) {
        return reply::error(StatusCode::GONE, "invite_invalid", e);
    }
    if let Err(e) = store.add_telegram_user(&username, data.user.id) {
        return reply::error(StatusCode::CONFLICT, "register_failed", e);
    }
    tracing::info!(user = %username, telegram_id = data.user.id, "user registered from Mini App invite");
    match store.account(&username, &config.admin_username) {
        Some(account) => session_reply(&config, &keys, &account, data.user.id),
        None => unauthorized(),
    }
}

#[get("/me")]
async fn me(user: web::ReqData<ApiUser>, config: web::Data<AppConfig>) -> HttpResponse {
    reply::ok(json!({
        "user": {
            "username": user.username,
            "is_admin": user.is_admin,
            "telegram_id": user.telegram_id,
        },
        "limits": limits(&config),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[post("/tg/unlink")]
async fn tg_unlink(
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    match store.unlink_telegram(user.telegram_id, &config.admin_username) {
        Ok(_) => {
            tracing::info!(user = %user.username, "telegram unlinked from Mini App");
            reply::ok(json!({ "message": "Telegram отвязан" }))
        }
        Err(_) => reply::error(
            StatusCode::NOT_FOUND,
            "not_linked",
            "Этот Telegram не привязан",
        ),
    }
}

#[post("/sessions/revoke-all")]
async fn revoke_all(
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    match store.bump_token_version(&user.username, &config.admin_username) {
        Ok(()) => reply::ok(json!({ "message": "Все сеансы мини-приложения завершены" })),
        Err(e) => reply::error(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e),
    }
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Item {
    File {
        id: String,
        scope: Zone,
        category: String,
        path: String,
        name: String,
        size: u64,
        mtime: Option<u64>,
        mime: String,
        kind: &'static str,
    },
    Folder {
        id: String,
        scope: Zone,
        category: String,
        path: String,
        name: String,
        files: u64,
        bytes: u64,
        folders: u64,
        mtime: Option<u64>,
    },
}

impl Item {
    fn is_folder(&self) -> bool {
        matches!(self, Item::Folder { .. })
    }

    fn name(&self) -> &str {
        match self {
            Item::File { name, .. } | Item::Folder { name, .. } => name,
        }
    }

    fn size(&self) -> u64 {
        match self {
            Item::File { size, .. } => *size,
            Item::Folder { bytes, .. } => *bytes,
        }
    }

    fn mtime(&self) -> u64 {
        match self {
            Item::File { mtime, .. } | Item::Folder { mtime, .. } => mtime.unwrap_or(0),
        }
    }
}

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "json", "yml", "yaml", "toml", "ini", "conf", "cfg", "log", "csv",
    "xml", "rs", "py", "sh", "js", "ts", "css", "go", "c", "h", "sql", "env",
];
const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "tar", "gz", "tgz", "7z", "rar", "xz", "bz2", "zst"];

fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

pub fn mime_for(name: &str) -> String {
    mime_guess::from_path(name)
        .first_or_octet_stream()
        .essence_str()
        .to_string()
}

pub fn kind_for(name: &str, mime: &str) -> &'static str {
    let ext = extension(name);
    if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("video/") {
        "video"
    } else if mime.starts_with("audio/") {
        "audio"
    } else if mime == "application/pdf" {
        "pdf"
    } else if ARCHIVE_EXTENSIONS.contains(&ext.as_str()) {
        "archive"
    } else if mime.starts_with("text/") || TEXT_EXTENSIONS.contains(&ext.as_str()) {
        "text"
    } else {
        "other"
    }
}

pub fn file_item(
    zone: Zone,
    category: &str,
    parent: &RelPath,
    name: &str,
    size: u64,
    mtime: Option<u64>,
) -> Item {
    let mime = mime_for(name);
    Item::File {
        id: encode_id(zone, category, parent, Some(name)),
        scope: zone,
        category: category.to_string(),
        path: parent.to_string(),
        name: name.to_string(),
        size,
        mtime,
        kind: kind_for(name, &mime),
        mime,
    }
}

fn folder_item(
    zone: Zone,
    category: &str,
    folder: &RelPath,
    dir: &Path,
    mtime: Option<u64>,
) -> Item {
    let stats = dir_stats(dir);
    let parent = folder.parent().unwrap_or_default();
    Item::Folder {
        id: encode_id(zone, category, folder, None),
        scope: zone,
        category: category.to_string(),
        path: parent.to_string(),
        name: folder.name().unwrap_or_default().to_string(),
        files: stats.files,
        bytes: stats.bytes,
        folders: stats.folders,
        mtime,
    }
}

/// Turn one directory entry into an item; entries whose names the API cannot
/// address (e.g. dot-folders made over SSH) are skipped.
fn entry_item(
    zone: Zone,
    category: &str,
    parent: &RelPath,
    dir: &Path,
    entry: &DirEntry,
) -> Option<Item> {
    if entry.is_dir {
        let folder = parent.join(&entry.name).ok()?;
        Some(folder_item(
            zone,
            category,
            &folder,
            &dir.join(&entry.name),
            entry.mtime,
        ))
    } else {
        exact_file_name(&entry.name).ok()?;
        Some(file_item(
            zone,
            category,
            parent,
            &entry.name,
            entry.size,
            entry.mtime,
        ))
    }
}

fn parse_zone(scope: &str) -> Result<Zone, HttpResponse> {
    Zone::parse(scope).ok_or_else(|| reply::storage(StorageError::Zone))
}

fn sort_items(items: &mut [Item], sort: Option<&str>, order: Option<&str>) {
    let desc = order == Some("desc");
    items.sort_by(|a, b| {
        // Folders always first, then the requested key.
        b.is_folder().cmp(&a.is_folder()).then_with(|| {
            let ord = match sort {
                Some("size") => a.size().cmp(&b.size()),
                Some("mtime") => a.mtime().cmp(&b.mtime()),
                _ => a.name().to_lowercase().cmp(&b.name().to_lowercase()),
            };
            if desc {
                ord.reverse()
            } else {
                ord
            }
        })
    });
}

#[derive(Deserialize)]
struct ListQuery {
    scope: String,
    category: Option<String>,
    path: Option<String>,
    q: Option<String>,
    sort: Option<String>,
    order: Option<String>,
}

/// List a folder (`category` + `path`), the roots of all categories (no
/// `category`), or search recursively (`q`).
#[get("/files")]
async fn list_files(
    query: web::Query<ListQuery>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
) -> HttpResponse {
    let zone = match parse_zone(&query.scope) {
        Ok(z) => z,
        Err(resp) => return resp,
    };
    let category = query.category.as_deref().filter(|c| !c.is_empty());
    let path = match RelPath::parse(query.path.as_deref().unwrap_or("")) {
        Ok(p) => p,
        Err(e) => return reply::bad_request(e),
    };
    if category.is_none() && !path.is_root() {
        return reply::bad_request("Путь задаётся внутри категории");
    }
    let categories: Vec<String> = match category {
        Some(c) if category_rel_dir(c).is_some() => vec![c.to_string()],
        Some(_) => return reply::storage(StorageError::Category),
        None => category_listing().into_iter().map(|(id, _)| id).collect(),
    };
    let needle = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_lowercase);
    let (cfg, username) = (config.clone(), user.username.clone());

    let result = web::block(move || -> Result<(Vec<Item>, bool), StorageError> {
        let mut items = Vec::new();
        let mut truncated = false;
        for category in &categories {
            let dir = folder_dir(&cfg, zone, &username, category, &path)?;
            match &needle {
                Some(needle) => {
                    let mut budget = WALK_LIMIT;
                    walk(
                        &dir,
                        path.segments().len(),
                        &mut budget,
                        &mut |rel, entry| {
                            if items.len() >= SEARCH_LIMIT {
                                truncated = true;
                                return;
                            }
                            if !entry.name.to_lowercase().contains(needle.as_str()) {
                                return;
                            }
                            let Ok(parent) = RelPath::parse(
                                &path
                                    .segments()
                                    .iter()
                                    .chain(rel)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join("/"),
                            ) else {
                                return;
                            };
                            let parent_dir = dir.join(rel.iter().collect::<std::path::PathBuf>());
                            if let Some(item) =
                                entry_item(zone, category, &parent, &parent_dir, entry)
                            {
                                items.push(item);
                            }
                        },
                    );
                }
                None => {
                    for entry in read_dir_entries(&dir)? {
                        if items.len() >= LIST_LIMIT {
                            truncated = true;
                            break;
                        }
                        if let Some(item) = entry_item(zone, category, &path, &dir, &entry) {
                            items.push(item);
                        }
                    }
                }
            }
        }
        Ok((items, truncated))
    })
    .await;

    match result {
        Ok(Ok((mut items, truncated))) => {
            sort_items(&mut items, query.sort.as_deref(), query.order.as_deref());
            reply::ok(json!({ "items": items, "truncated": truncated }))
        }
        Ok(Err(e)) => reply::storage(e),
        Err(_) => reply::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Внутренняя ошибка",
        ),
    }
}

#[get("/overview")]
async fn overview(user: web::ReqData<ApiUser>, config: web::Data<AppConfig>) -> HttpResponse {
    let (cfg, username, is_admin) = (config.clone(), user.username.clone(), user.is_admin);
    let result = web::block(move || {
        let mut zones = Vec::new();
        let mut recent: Vec<Item> = Vec::new();
        for zone in [Zone::My, Zone::Shared] {
            let mut categories = Vec::new();
            let (mut files, mut bytes) = (0u64, 0u64);
            for (id, title) in category_listing() {
                let Ok(dir) = category_dir(&cfg, zone, &username, &id) else {
                    continue;
                };
                let stats = dir_stats(&dir);
                files += stats.files;
                bytes += stats.bytes;
                categories.push(json!({
                    "category": id, "title": title,
                    "files": stats.files, "folders": stats.folders, "bytes": stats.bytes,
                }));
                let mut budget = WALK_LIMIT;
                walk(&dir, 0, &mut budget, &mut |rel, entry| {
                    if entry.is_dir {
                        return;
                    }
                    let Ok(parent) = RelPath::parse(&rel.join("/")) else {
                        return;
                    };
                    if exact_file_name(&entry.name).is_ok() {
                        recent.push(file_item(
                            zone,
                            &id,
                            &parent,
                            &entry.name,
                            entry.size,
                            entry.mtime,
                        ));
                    }
                });
                // Keep memory bounded while walking big trees.
                if recent.len() > RECENT_LIMIT * 20 {
                    recent.sort_by_key(|i| std::cmp::Reverse(i.mtime()));
                    recent.truncate(RECENT_LIMIT);
                }
            }
            zones.push(json!({
                "scope": zone,
                "title": if zone == Zone::My { "Мои файлы" } else { "Общие файлы" },
                "files": files, "bytes": bytes, "categories": categories,
            }));
        }
        recent.sort_by_key(|i| std::cmp::Reverse(i.mtime()));
        recent.truncate(RECENT_LIMIT);
        let disk = is_admin
            .then(|| disk_usage(&cfg.upload_dir))
            .flatten()
            .map(|(total, free)| json!({ "total_bytes": total, "free_bytes": free }));
        json!({ "zones": zones, "recent": recent, "disk": disk })
    })
    .await;
    match result {
        Ok(body) => reply::ok(body),
        Err(_) => reply::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Внутренняя ошибка",
        ),
    }
}

/// Resolve an item id for this user: the path on disk and the decoded parts.
fn resolve(
    config: &AppConfig,
    user: &ApiUser,
    id: &str,
) -> Result<(ItemRef, std::path::PathBuf), StorageError> {
    let item = decode_id(id)?;
    let path = match &item.name {
        Some(name) => file_path(
            config,
            item.zone,
            &user.username,
            &item.category,
            &item.path,
            name,
        )?,
        None => folder_dir(
            config,
            item.zone,
            &user.username,
            &item.category,
            &item.path,
        )?,
    };
    Ok((item, path))
}

fn describe(item: &ItemRef, path: &Path) -> Result<Item, StorageError> {
    let meta = fs::symlink_metadata(path).map_err(StorageError::from)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    match &item.name {
        Some(name) if meta.is_file() => Ok(file_item(
            item.zone,
            &item.category,
            &item.path,
            name,
            meta.len(),
            mtime,
        )),
        None if meta.is_dir() => Ok(folder_item(
            item.zone,
            &item.category,
            &item.path,
            path,
            mtime,
        )),
        _ => Err(StorageError::NotFound),
    }
}

#[get("/files/{id}")]
async fn item_details(
    id: web::Path<String>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
) -> HttpResponse {
    match resolve(&config, &user, &id).and_then(|(item, path)| describe(&item, &path)) {
        Ok(item) => reply::ok(json!({ "item": item })),
        Err(e) => reply::storage(e),
    }
}

#[derive(Deserialize)]
struct RenameBody {
    new_name: String,
}

#[post("/files/{id}/rename")]
async fn rename_item(
    id: web::Path<String>,
    body: web::Json<RenameBody>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
) -> HttpResponse {
    let result = (|| -> Result<Item, StorageError> {
        let (item, old_path) = resolve(&config, &user, &id)?;
        match &item.name {
            Some(old_name) => {
                if !old_path.is_file() {
                    return Err(StorageError::NotFound);
                }
                let new_name = exact_file_name(&body.new_name)?.to_string();
                let new_path = file_path(
                    &config,
                    item.zone,
                    &user.username,
                    &item.category,
                    &item.path,
                    &new_name,
                )?;
                // Hard link + remove: fails instead of overwriting an existing file.
                fs::hard_link(&old_path, &new_path).map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        StorageError::Exists
                    } else {
                        StorageError::from(e)
                    }
                })?;
                fs::remove_file(&old_path)?;
                tracing::info!(user = %user.username, from = %old_name, to = %new_name, "file renamed (Mini App)");
                describe(
                    &ItemRef {
                        name: Some(new_name),
                        ..item
                    },
                    &new_path,
                )
            }
            None => {
                let (new_rel, new_path) = storage::rename_folder(
                    &config,
                    item.zone,
                    &user.username,
                    &item.category,
                    &item.path,
                    &body.new_name,
                )?;
                tracing::info!(user = %user.username, to = %new_rel, "folder renamed (Mini App)");
                describe(
                    &ItemRef {
                        path: new_rel,
                        ..item
                    },
                    &new_path,
                )
            }
        }
    })();
    match result {
        Ok(item) => reply::ok(json!({ "item": item })),
        Err(e) => reply::storage(e),
    }
}

#[delete("/files/{id}")]
async fn delete_item(
    id: web::Path<String>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
) -> HttpResponse {
    let result = (|| -> Result<&'static str, StorageError> {
        let (item, path) = resolve(&config, &user, &id)?;
        if item.name.is_some() {
            if !path.is_file() {
                return Err(StorageError::NotFound);
            }
            fs::remove_file(&path)?;
            tracing::info!(user = %user.username, scope = item.zone.as_str(), category = %item.category, "file deleted (Mini App)");
            Ok("Файл удалён")
        } else {
            storage::remove_empty_folder(&path)?;
            Ok("Папка удалена")
        }
    })();
    match result {
        Ok(message) => reply::ok(json!({ "message": message })),
        Err(e) => reply::storage(e),
    }
}

#[derive(Deserialize)]
struct NewFolderBody {
    scope: String,
    category: String,
    #[serde(default)]
    path: String,
    name: String,
}

#[post("/folders")]
async fn create_folder(
    body: web::Json<NewFolderBody>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
) -> HttpResponse {
    let result = (|| -> Result<Item, StorageError> {
        let zone = Zone::parse(&body.scope).ok_or(StorageError::Zone)?;
        let parent = RelPath::parse(&body.path).map_err(StorageError::Path)?;
        let (folder, dir) = storage::create_folder(
            &config,
            zone,
            &user.username,
            &body.category,
            &parent,
            &body.name,
        )?;
        tracing::info!(user = %user.username, scope = zone.as_str(), category = %body.category, folder = %folder, "folder created");
        describe(
            &ItemRef {
                zone,
                category: body.category.clone(),
                path: folder,
                name: None,
            },
            &dir,
        )
    })();
    match result {
        Ok(item) => reply::ok(json!({ "item": item })),
        Err(e) => reply::storage(e),
    }
}

#[derive(Deserialize)]
struct LinkRequest {
    #[serde(default)]
    purpose: Option<String>,
}

/// Signed URL for `<img>`, `<video>` and Telegram's native download dialog.
#[post("/files/{id}/link")]
async fn make_link(
    id: web::Path<String>,
    body: web::Json<LinkRequest>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    keys: web::Data<TokenKeys>,
) -> HttpResponse {
    let (item, path) = match resolve(&config, &user, &id) {
        Ok(v) => v,
        Err(e) => return reply::storage(e),
    };
    let Some(name) = item.name.clone() else {
        return reply::bad_request("Ссылку можно получить только на файл");
    };
    if !path.is_file() {
        return reply::storage(StorageError::NotFound);
    }
    let (disposition, ttl) = match body.purpose.as_deref() {
        Some("inline") | Some("stream") => (Disposition::Inline, STREAM_URL_TTL),
        _ => (Disposition::Attachment, DOWNLOAD_URL_TTL),
    };
    let Some(account) = store.account(&user.username, &config.admin_username) else {
        return unauthorized();
    };
    let exp = now_secs() + ttl;
    let token = sign_url(
        &keys,
        &UrlClaims {
            u: user.username.clone(),
            tv: account.token_version,
            z: item.zone,
            c: item.category,
            p: item.path.to_string(),
            n: name,
            d: disposition,
            exp,
        },
    );
    reply::ok(json!({ "url": format!("/d/{token}"), "expires_at": exp }))
}

/// `filename*` (RFC 6266/5987) carries the real UTF-8 name; plain `filename`
/// is an ASCII fallback for old clients, so Cyrillic names survive downloads.
fn disposition(kind: DispositionType, name: &str) -> ContentDisposition {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() && c != '"' {
                c
            } else {
                '_'
            }
        })
        .collect();
    ContentDisposition {
        disposition: kind,
        parameters: vec![
            DispositionParam::Filename(ascii),
            DispositionParam::FilenameExt(ExtendedValue {
                charset: Charset::Ext("UTF-8".into()),
                language_tag: None,
                value: name.as_bytes().to_vec(),
            }),
        ],
    }
}

/// Types a browser may render inline from a signed link. Never SVG or HTML.
fn inline_safe(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg"
            | "image/png"
            | "image/gif"
            | "image/webp"
            | "image/avif"
            | "video/mp4"
            | "video/webm"
            | "application/pdf"
    ) || mime.starts_with("audio/")
}

// HEAD too: Telegram's WebApp.downloadFile asks for the size with HEAD before
// downloading. Without it HEAD fell through to the web UI's login guard and
// Telegram showed the size of its 401 JSON (57 bytes) instead of the file's.
#[route("/d/{token}", method = "GET", method = "HEAD")]
async fn signed_download(
    req: HttpRequest,
    token: web::Path<String>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    keys: web::Data<TokenKeys>,
) -> Result<HttpResponse, Error> {
    let expired = || {
        reply::error(
            StatusCode::GONE,
            "expired",
            "Ссылка устарела — откройте файл заново",
        )
    };
    let Some(claims) = verify_url(&keys, &token, now_secs()) else {
        return Ok(expired());
    };
    if store
        .account(&claims.u, &config.admin_username)
        .is_none_or(|a| a.token_version != claims.tv)
    {
        return Ok(expired());
    }
    let path = match RelPath::parse(&claims.p)
        .map_err(StorageError::Path)
        .and_then(|rel| file_path(&config, claims.z, &claims.u, &claims.c, &rel, &claims.n))
    {
        Ok(p) if p.is_file() => p,
        Ok(_) => return Ok(reply::storage(StorageError::NotFound)),
        Err(e) => return Ok(reply::storage(e)),
    };
    let file = NamedFile::open(&path)?;
    let mime = file.content_type().essence_str().to_string();
    let inline = claims.d == Disposition::Inline && inline_safe(&mime);
    let mut response = file
        .set_content_disposition(disposition(
            if inline {
                DispositionType::Inline
            } else {
                DispositionType::Attachment
            },
            &claims.n,
        ))
        .use_last_modified(true)
        .into_response(&req);
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    // Even if a browser renders the file, it gets no scripts and no origin.
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        header::HeaderValue::from_static("default-src 'none'; img-src 'self'; media-src 'self'; style-src 'unsafe-inline'; sandbox"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, max-age=300"),
    );
    // Telegram Web downloads through fetch from its own origin (WebApp.downloadFile).
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::HeaderValue::from_static("https://web.telegram.org"),
    );
    Ok(response)
}

/// Category for a new upload: explicit or derived from the extension.
pub fn upload_category(explicit: Option<&str>, name: &str) -> Result<String, StorageError> {
    match explicit.filter(|c| !c.is_empty()) {
        Some(c) => category_rel_dir(c)
            .map(|_| c.to_string())
            .ok_or(StorageError::Category),
        None => Ok(category_for_extension(&extension(name)).to_string()),
    }
}

/// Destination folder for a finished upload, created (without following
/// symlinks) if missing; then the staged file is published without overwriting.
pub fn finish_upload(
    config: &AppConfig,
    zone: Zone,
    username: &str,
    category: &str,
    rel: &RelPath,
    staged: &Path,
    name: &str,
) -> Result<Item, StorageError> {
    let dir = folder_dir(config, zone, username, category, rel)?;
    fs::create_dir_all(&dir)?;
    // Re-check after creation: nothing on the way may have become a symlink.
    folder_dir(config, zone, username, category, rel)?;
    let target = publish_file(staged, &dir, name)?;
    let final_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(name)
        .to_string();
    let meta = fs::metadata(&target)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    Ok(file_item(
        zone,
        category,
        rel,
        &final_name,
        meta.len(),
        mtime,
    ))
}
