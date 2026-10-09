//! «Обмен» — a private space between the admin and each user.
//!
//! `UPLOAD_DIR/exchange/<user>/from-admin/` holds what the admin sent to
//! `<user>`, `from-user/` what `<user>` sent to the admin. Nobody else sees
//! it: a user only ever reaches their own exchange (the name comes from the
//! session or token, never from the request), the admin reaches the exchange
//! of any existing user. Both sides can download, delete and save a copy into
//! their own files. A copy is a hard link: instant and without extra disk use.
//!
//! The web UI routes (cookie session) and the Mini App routes (Bearer token)
//! both live here and share the same functions.

use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use actix_files::NamedFile;
use actix_multipart::Multipart;
use actix_session::Session;
use actix_web::http::header::DispositionType;
use actix_web::http::StatusCode;
use actix_web::{delete, get, post, web, Error, HttpRequest, HttpResponse};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{kind_for, mime_for, ApiUser};
use crate::auth::{current_user, CurrentUser};
use crate::categories::{category_for_extension, sanitize_file_name};
use crate::config::AppConfig;
use crate::paths::{ensure_no_symlinks, RelPath};
use crate::reply::{self, now_secs};
use crate::storage::{
    category_dir, encode_url_segment, exact_file_name, format_bytes, link_file, read_dir_entries,
    DirEntry, PendingUpload, StorageError, Zone,
};
use crate::tokens::{sign_url, Disposition, TokenKeys, UrlClaims};
use crate::users::UserStore;

pub const EXCHANGE_DIR: &str = "exchange";
/// Signed links to exchange files live as long as other download links.
const LINK_TTL: u64 = 5 * 60;
const STREAM_TTL: u64 = 6 * 60 * 60;

/// How many recent arrivals the bot can still fetch.
const MAX_EVENTS: usize = 500;

/// A file that arrived in an exchange; the bot turns it into a notification.
#[derive(Clone, Debug)]
pub struct Event {
    pub id: u64,
    pub ts: u64,
    pub owner: String,
    pub inbox: Inbox,
    pub name: String,
    pub size: u64,
}

/// Recent arrivals, kept in memory only. Ids are microseconds since the epoch
/// (strictly increasing), so after a restart new ids still follow the ones the
/// bot has seen; an arrival in the seconds before a restart may go unannounced,
/// which is acceptable for a notification.
pub struct Events {
    inner: Mutex<(VecDeque<Event>, u64)>,
}

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

impl Default for Events {
    fn default() -> Self {
        Events {
            inner: Mutex::new((VecDeque::new(), now_micros())),
        }
    }
}

impl Events {
    pub fn push(&self, owner: &str, inbox: Inbox, name: &str, size: u64) {
        let mut guard = self.inner.lock().expect("exchange events");
        let (queue, last) = &mut *guard;
        let id = now_micros().max(*last + 1);
        *last = id;
        queue.push_back(Event {
            id,
            ts: id / 1_000_000,
            owner: owner.to_string(),
            inbox,
            name: name.to_string(),
            size,
        });
        if queue.len() > MAX_EVENTS {
            queue.pop_front();
        }
    }

    /// Events with an id above `after`, oldest first, at most `limit`.
    pub fn after(&self, after: u64, limit: usize) -> Vec<Event> {
        let guard = self.inner.lock().expect("exchange events");
        guard
            .0
            .iter()
            .filter(|e| e.id > after)
            .take(limit)
            .cloned()
            .collect()
    }

    /// The newest id handed out (or the start time): where a fresh reader begins.
    pub fn last_id(&self) -> u64 {
        self.inner.lock().expect("exchange events").1
    }
}

/// One direction of an exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inbox {
    /// Files the admin sent to the user.
    FromAdmin,
    /// Files the user sent to the admin.
    FromUser,
}

impl Inbox {
    pub fn as_str(self) -> &'static str {
        match self {
            Inbox::FromAdmin => "from-admin",
            Inbox::FromUser => "from-user",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "from-admin" => Some(Inbox::FromAdmin),
            "from-user" => Some(Inbox::FromUser),
            _ => None,
        }
    }
}

/// Who looks at an exchange: the user it belongs to, or the admin.
#[derive(Clone, Copy)]
pub struct Viewer<'a> {
    pub username: &'a str,
    pub is_admin: bool,
}

impl Viewer<'_> {
    /// The direction this viewer sends into.
    pub fn outgoing(&self) -> Inbox {
        if self.is_admin {
            Inbox::FromAdmin
        } else {
            Inbox::FromUser
        }
    }

    pub fn direction(&self, inbox: Inbox) -> &'static str {
        if inbox == self.outgoing() {
            "outgoing"
        } else {
            "incoming"
        }
    }
}

impl<'a> From<&'a CurrentUser> for Viewer<'a> {
    fn from(user: &'a CurrentUser) -> Self {
        Viewer {
            username: &user.username,
            is_admin: user.is_admin,
        }
    }
}

impl<'a> From<&'a ApiUser> for Viewer<'a> {
    fn from(user: &'a ApiUser) -> Self {
        Viewer {
            username: &user.username,
            is_admin: user.is_admin,
        }
    }
}

/// The user whose exchange `with` names for this viewer. A user reaches only
/// their own (an empty `with` means "mine"); the admin reaches any existing
/// user. Everything else is "not found", so names of others do not leak.
pub fn owner(store: &UserStore, viewer: Viewer, with: &str) -> Result<String, StorageError> {
    if viewer.is_admin {
        if !with.is_empty() && store.user_exists(with) {
            return Ok(with.to_string());
        }
    } else if with.is_empty() || with == viewer.username {
        return Ok(viewer.username.to_string());
    }
    Err(StorageError::NotFound)
}

fn root(config: &AppConfig) -> PathBuf {
    config.upload_dir.join(EXCHANGE_DIR)
}

fn inbox_rel(owner: &str, inbox: Inbox) -> Result<RelPath, StorageError> {
    RelPath::parse(&format!("{owner}/{}", inbox.as_str())).map_err(StorageError::Path)
}

/// Directory of one direction (not created); symlinks on the way are refused.
pub fn inbox_dir(config: &AppConfig, owner: &str, inbox: Inbox) -> Result<PathBuf, StorageError> {
    let rel = inbox_rel(owner, inbox)?;
    let base = root(config);
    ensure_no_symlinks(&base, &rel, None)?;
    Ok(base.join(rel.to_path_buf()))
}

/// A file of an exchange; the name must be exact and nothing may be a symlink.
pub fn file_path(
    config: &AppConfig,
    owner: &str,
    inbox: Inbox,
    name: &str,
) -> Result<PathBuf, StorageError> {
    let name = exact_file_name(name)?;
    let rel = inbox_rel(owner, inbox)?;
    let base = root(config);
    ensure_no_symlinks(&base, &rel, Some(name))?;
    Ok(base.join(rel.to_path_buf()).join(name))
}

/// Where this viewer's files for `owner`'s exchange go, created on demand.
fn outgoing_dir(config: &AppConfig, owner: &str, viewer: Viewer) -> Result<PathBuf, StorageError> {
    let dir = inbox_dir(config, owner, viewer.outgoing())?;
    fs::create_dir_all(&dir)?;
    // Re-check after creation: nothing on the way may have become a symlink.
    inbox_dir(config, owner, viewer.outgoing())?;
    Ok(dir)
}

/// Files of one direction, newest first.
fn files_in(config: &AppConfig, owner: &str, inbox: Inbox) -> Vec<DirEntry> {
    let mut files: Vec<DirEntry> = inbox_dir(config, owner, inbox)
        .ok()
        .and_then(|dir| read_dir_entries(&dir).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|e| !e.is_dir && exact_file_name(&e.name).is_ok())
        .collect();
    files.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.name.cmp(&b.name)));
    files
}

/// Incoming and outgoing files of one exchange from the viewer's side.
pub struct Listing {
    pub owner: String,
    pub incoming: Vec<DirEntry>,
    pub outgoing: Vec<DirEntry>,
    pub incoming_inbox: Inbox,
    pub outgoing_inbox: Inbox,
}

impl Listing {
    pub fn last_activity(&self) -> u64 {
        self.incoming
            .iter()
            .chain(&self.outgoing)
            .filter_map(|e| e.mtime)
            .max()
            .unwrap_or(0)
    }
}

pub fn listing(config: &AppConfig, owner: &str, viewer: Viewer) -> Listing {
    let outgoing_inbox = viewer.outgoing();
    let incoming_inbox = match outgoing_inbox {
        Inbox::FromAdmin => Inbox::FromUser,
        Inbox::FromUser => Inbox::FromAdmin,
    };
    Listing {
        owner: owner.to_string(),
        incoming: files_in(config, owner, incoming_inbox),
        outgoing: files_in(config, owner, outgoing_inbox),
        incoming_inbox,
        outgoing_inbox,
    }
}

/// The admin's view: one exchange per user, the most recently active first.
pub fn partners(config: &AppConfig, store: &UserStore, viewer: Viewer) -> Vec<Listing> {
    let mut all: Vec<Listing> = store
        .list_users()
        .into_iter()
        .map(|u| listing(config, &u.username, viewer))
        .collect();
    all.sort_by(|a, b| {
        b.last_activity()
            .cmp(&a.last_activity())
            .then_with(|| a.owner.cmp(&b.owner))
    });
    all
}

fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// Copy an exchange file into the viewer's own files, category by file type.
/// Returns the category and the final name (a taken name gets `(1)`).
pub fn save_copy(
    config: &AppConfig,
    viewer: Viewer,
    source: &Path,
    name: &str,
) -> Result<(String, String), StorageError> {
    let category = category_for_extension(&extension(name)).to_string();
    let dir = category_dir(config, Zone::My, viewer.username, &category)?;
    fs::create_dir_all(&dir)?;
    let target = link_file(source, &dir, name)?;
    let saved = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(name)
        .to_string();
    Ok((category, saved))
}

/// Remove a whole exchange (when its user is deleted).
pub fn remove_all(config: &AppConfig, owner: &str) {
    let Ok(rel) = RelPath::parse(owner) else {
        return;
    };
    let base = root(config);
    if ensure_no_symlinks(&base, &rel, None).is_err() {
        return;
    }
    let dir = base.join(rel.to_path_buf());
    if dir.is_dir() {
        if let Err(e) = fs::remove_dir_all(&dir) {
            tracing::warn!(user = %owner, error = %e, "failed to remove exchange dir");
        }
    }
}

// ---------------------------------------------------------------------------
// Web UI (cookie session)
// ---------------------------------------------------------------------------

fn web_row(owner: &str, inbox: Inbox, incoming: bool, entry: &DirEntry) -> Value {
    json!({
        "owner": owner,
        "inbox": inbox.as_str(),
        "incoming": incoming,
        "name": entry.name,
        "size": format_bytes(entry.size),
        "size_bytes": entry.size,
        "modified": entry.mtime,
        "url": format!(
            "/exchange/{}/{}/{}",
            encode_url_segment(owner),
            inbox.as_str(),
            encode_url_segment(&entry.name)
        ),
    })
}

/// One exchange for the page, with labels from the viewer's side.
fn web_listing(listing: &Listing, viewer_is_admin: bool) -> Value {
    let owner = &listing.owner;
    let (incoming_title, outgoing_title, send_label) = if viewer_is_admin {
        (
            format!("От {owner}"),
            format!("Отправлено {owner}"),
            format!("Отправить файлы участнику {owner}"),
        )
    } else {
        (
            "От администратора".to_string(),
            "Отправлено администратору".to_string(),
            "Отправить файлы администратору".to_string(),
        )
    };
    json!({
        "owner": owner,
        "incoming": listing.incoming.iter().map(|e| web_row(owner, listing.incoming_inbox, true, e)).collect::<Vec<_>>(),
        "outgoing": listing.outgoing.iter().map(|e| web_row(owner, listing.outgoing_inbox, false, e)).collect::<Vec<_>>(),
        "incoming_count": listing.incoming.len(),
        "outgoing_count": listing.outgoing.len(),
        "total": listing.incoming.len() + listing.outgoing.len(),
        "incoming_title": incoming_title,
        "outgoing_title": outgoing_title,
        "send_label": send_label,
    })
}

/// Template data of the «Обмен» section for the page.
pub fn page_data(config: &AppConfig, store: &UserStore, user: &CurrentUser) -> Value {
    let viewer = Viewer::from(user);
    if viewer.is_admin {
        let partners = partners(config, store, viewer);
        let incoming: usize = partners.iter().map(|p| p.incoming.len()).sum();
        json!({
            "is_admin": true,
            "partners": partners.iter().map(|p| web_listing(p, true)).collect::<Vec<_>>(),
            "incoming_count": incoming,
        })
    } else {
        let mine = listing(config, &user.username, viewer);
        let mut data = web_listing(&mine, false);
        data["is_admin"] = json!(false);
        data
    }
}

fn web_error(e: StorageError) -> HttpResponse {
    reply::storage(e)
}

fn session_user(session: &Session) -> Result<CurrentUser, HttpResponse> {
    current_user(session)
        .ok_or_else(|| reply::error(StatusCode::UNAUTHORIZED, "unauthorized", "Требуется вход"))
}

/// Resolve `{owner}/{inbox}/{name}` of a web request to a file path.
fn web_file(
    config: &AppConfig,
    store: &UserStore,
    user: &CurrentUser,
    owner_name: &str,
    inbox: &str,
    name: &str,
) -> Result<(String, Inbox, PathBuf), HttpResponse> {
    let owner = owner(store, Viewer::from(user), owner_name).map_err(web_error)?;
    let inbox =
        Inbox::parse(inbox).ok_or_else(|| reply::bad_request("Неизвестная папка обмена"))?;
    let path = file_path(config, &owner, inbox, name).map_err(web_error)?;
    if !path.is_file() {
        return Err(web_error(StorageError::NotFound));
    }
    Ok((owner, inbox, path))
}

fn partner_label(user: &CurrentUser, owner: &str) -> String {
    if user.is_admin {
        owner.to_string()
    } else {
        "администратор".into()
    }
}

/// Send files: the admin into `from-admin` of that user, a user into their
/// own `from-user`. Names never overwrite (a taken name gets `(1)`).
#[post("/exchange/{owner}/upload")]
pub async fn web_upload(
    owner_name: web::Path<String>,
    mut payload: Multipart,
    session: Session,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    events: web::Data<Events>,
) -> Result<HttpResponse, Error> {
    let user = match session_user(&session) {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    let viewer = Viewer::from(&user);
    let target = match owner(&store, viewer, &owner_name)
        .and_then(|owner| outgoing_dir(&config, &owner, viewer).map(|dir| (owner, dir)))
    {
        Ok(t) => t,
        Err(e) => return Ok(web_error(e)),
    };
    let (owner, dir) = target;
    let mut sent = Vec::new();
    while let Some(item) = payload.next().await {
        let mut field = item?;
        let raw_name = field
            .content_disposition()
            .and_then(|cd| cd.get_filename())
            .unwrap_or("")
            .to_string();
        let Some(file_name) = sanitize_file_name(&raw_name) else {
            return Ok(reply::bad_request("Недопустимое имя файла"));
        };
        if exact_file_name(&file_name).is_err() {
            return Ok(reply::bad_request("Недопустимое имя файла"));
        }
        // Reserve the name atomically; an interrupted upload leaves nothing.
        let mut pending = PendingUpload::create(&dir, &file_name)?;
        let mut size = 0usize;
        while let Some(chunk) = field.next().await {
            let data = chunk?;
            size += data.len();
            if size > config.max_file_size {
                return Ok(reply::error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "too_large",
                    format!(
                        "Файл превышает лимит {}",
                        format_bytes(config.max_file_size as u64)
                    ),
                ));
            }
            pending
                .file
                .as_mut()
                .expect("upload file is open")
                .write_all(&data)?;
        }
        pending.finish()?;
        let saved = pending
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        tracing::info!(user = %user.username, exchange = %owner, file = %saved, size, "file sent to exchange");
        events.push(&owner, viewer.outgoing(), &saved, size as u64);
        sent.push(saved);
    }
    match sent.as_slice() {
        [] => Ok(reply::bad_request("Файл не получен")),
        [one] => Ok(reply::ok(json!({
            "message": format!("Отправлено ({}): {one}", partner_label(&user, &owner)),
        }))),
        many => Ok(reply::ok(json!({
            "message": format!("Отправлено файлов ({}): {}", partner_label(&user, &owner), many.len()),
        }))),
    }
}

#[get("/exchange/{owner}/{inbox}/{name}")]
pub async fn web_download(
    req: HttpRequest,
    path: web::Path<(String, String, String)>,
    session: Session,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> Result<HttpResponse, Error> {
    let user = match session_user(&session) {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    let (owner_name, inbox, name) = path.into_inner();
    let (_, _, file) = match web_file(&config, &store, &user, &owner_name, &inbox, &name) {
        Ok(v) => v,
        Err(resp) => return Ok(resp),
    };
    // Attachment always: user-controlled HTML/SVG must not run under our origin.
    Ok(NamedFile::open(file)?
        .set_content_disposition(crate::api::disposition(DispositionType::Attachment, &name))
        .use_last_modified(true)
        .into_response(&req))
}

#[delete("/exchange/{owner}/{inbox}/{name}")]
pub async fn web_delete(
    path: web::Path<(String, String, String)>,
    session: Session,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    let user = match session_user(&session) {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let (owner_name, inbox, name) = path.into_inner();
    match web_file(&config, &store, &user, &owner_name, &inbox, &name) {
        Ok((owner, _, file)) => match fs::remove_file(&file) {
            Ok(()) => {
                tracing::info!(user = %user.username, exchange = %owner, file = %name, "exchange file deleted");
                reply::ok(json!({ "message": format!("Удалено из обмена: {name}") }))
            }
            Err(e) => web_error(e.into()),
        },
        Err(resp) => resp,
    }
}

#[post("/exchange/{owner}/{inbox}/{name}/save")]
pub async fn web_save(
    path: web::Path<(String, String, String)>,
    session: Session,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    let user = match session_user(&session) {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let (owner_name, inbox, name) = path.into_inner();
    let result = web_file(&config, &store, &user, &owner_name, &inbox, &name).and_then(
        |(owner, _, file)| {
            save_copy(&config, Viewer::from(&user), &file, &name)
                .map(|saved| (owner, saved))
                .map_err(web_error)
        },
    );
    match result {
        Ok((owner, (category, saved))) => {
            tracing::info!(user = %user.username, exchange = %owner, file = %saved, category = %category, "exchange file saved to own files");
            reply::ok(json!({
                "message": format!("Сохранено в «Мои файлы › {category}»: {saved}"),
                "category": category,
                "name": saved,
            }))
        }
        Err(resp) => resp,
    }
}

// ---------------------------------------------------------------------------
// Mini App API (Bearer token)
// ---------------------------------------------------------------------------

pub fn configure_api(cfg: &mut web::ServiceConfig) {
    cfg.service(api_overview)
        .service(api_listing)
        .service(api_link)
        .service(api_save)
        .service(api_delete);
}

/// Opaque-looking id of an exchange file: base64url of `owner \0 inbox \0 name`.
/// Not secret — every use re-checks the caller's right to that exchange.
pub fn encode_item(owner: &str, inbox: Inbox, name: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{owner}\0{}\0{name}", inbox.as_str()))
}

fn decode_item(id: &str) -> Result<(String, Inbox, String), StorageError> {
    let raw = URL_SAFE_NO_PAD
        .decode(id)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .ok_or(StorageError::NotFound)?;
    let mut parts = raw.splitn(3, '\0');
    let (Some(owner), Some(inbox), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(StorageError::NotFound);
    };
    let inbox = Inbox::parse(inbox).ok_or(StorageError::NotFound)?;
    Ok((owner.to_string(), inbox, exact_file_name(name)?.to_string()))
}

pub fn api_item(owner: &str, inbox: Inbox, viewer: Viewer, entry: &DirEntry) -> Value {
    let mime = mime_for(&entry.name);
    json!({
        "id": encode_item(owner, inbox, &entry.name),
        "owner": owner,
        "direction": viewer.direction(inbox),
        "name": entry.name,
        "size": entry.size,
        "mtime": entry.mtime,
        "kind": kind_for(&entry.name, &mime),
        "mime": mime,
    })
}

fn api_listing_json(listing: &Listing, viewer: Viewer) -> Value {
    let items = |inbox: Inbox, entries: &[DirEntry]| {
        entries
            .iter()
            .map(|e| api_item(&listing.owner, inbox, viewer, e))
            .collect::<Vec<_>>()
    };
    json!({
        "owner": listing.owner,
        "incoming": items(listing.incoming_inbox, &listing.incoming),
        "outgoing": items(listing.outgoing_inbox, &listing.outgoing),
    })
}

/// A user gets their exchange with the admin; the admin gets one summary
/// per user (open one with `GET /exchange/{owner}`).
#[get("/exchange")]
async fn api_overview(
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    let viewer = Viewer::from(&*user);
    if viewer.is_admin {
        let partners: Vec<Value> = partners(&config, &store, viewer)
            .iter()
            .map(|p| {
                json!({
                    "username": p.owner,
                    "incoming": p.incoming.len(),
                    "outgoing": p.outgoing.len(),
                    "last_mtime": p.last_activity(),
                })
            })
            .collect();
        reply::ok(json!({ "role": "admin", "partners": partners }))
    } else {
        let mut body = api_listing_json(&listing(&config, &user.username, viewer), viewer);
        body["role"] = json!("user");
        reply::ok(body)
    }
}

#[get("/exchange/{owner}")]
async fn api_listing(
    owner_name: web::Path<String>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    let viewer = Viewer::from(&*user);
    match owner(&store, viewer, &owner_name) {
        Ok(owner) => reply::ok(api_listing_json(&listing(&config, &owner, viewer), viewer)),
        Err(e) => reply::storage(e),
    }
}

/// Resolve an item id for this caller: their right to the exchange first.
pub(crate) fn api_file(
    config: &AppConfig,
    store: &UserStore,
    user: &ApiUser,
    id: &str,
) -> Result<(String, Inbox, String, PathBuf), StorageError> {
    let (owner_name, inbox, name) = decode_item(id)?;
    let owner = owner(store, Viewer::from(user), &owner_name)?;
    let path = file_path(config, &owner, inbox, &name)?;
    if !path.is_file() {
        return Err(StorageError::NotFound);
    }
    Ok((owner, inbox, name, path))
}

#[derive(Deserialize)]
struct LinkBody {
    #[serde(default)]
    purpose: Option<String>,
}

/// Signed `/d/` link for downloads and previews, like for own files.
#[post("/exchange/items/{id}/link")]
async fn api_link(
    id: web::Path<String>,
    body: web::Json<LinkBody>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    keys: web::Data<TokenKeys>,
) -> HttpResponse {
    let (owner, inbox, name, _) = match api_file(&config, &store, &user, &id) {
        Ok(v) => v,
        Err(e) => return reply::storage(e),
    };
    let Some(account) = store.account(&user.username, &config.admin_username) else {
        return reply::storage(StorageError::NotFound);
    };
    let (d, ttl) = match body.purpose.as_deref() {
        Some("inline") | Some("stream") => (Disposition::Inline, STREAM_TTL),
        _ => (Disposition::Attachment, LINK_TTL),
    };
    let exp = now_secs() + ttl;
    let token = sign_url(
        &keys,
        &UrlClaims {
            u: user.username.clone(),
            tv: account.token_version,
            z: Zone::My,
            c: inbox.as_str().to_string(),
            p: String::new(),
            n: name,
            d,
            exp,
            x: Some(owner),
        },
    );
    reply::ok(json!({ "url": format!("/d/{token}"), "expires_at": exp }))
}

/// File behind a signed exchange link, if its user may still open it.
pub fn link_target(
    config: &AppConfig,
    store: &UserStore,
    claims: &UrlClaims,
    owner_name: &str,
) -> Result<PathBuf, StorageError> {
    let viewer = Viewer {
        username: &claims.u,
        is_admin: claims.u == config.admin_username,
    };
    let owner = owner(store, viewer, owner_name)?;
    let inbox = Inbox::parse(&claims.c).ok_or(StorageError::NotFound)?;
    file_path(config, &owner, inbox, &claims.n)
}

#[post("/exchange/items/{id}/save")]
async fn api_save(
    id: web::Path<String>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    let result = api_file(&config, &store, &user, &id).and_then(|(owner, _, name, path)| {
        save_copy(&config, Viewer::from(&*user), &path, &name).map(|saved| (owner, saved))
    });
    match result {
        Ok((owner, (category, saved))) => {
            tracing::info!(user = %user.username, exchange = %owner, file = %saved, category = %category, "exchange file saved to own files (Mini App)");
            reply::ok(json!({
                "message": format!("Сохранено в «Мои файлы › {category}»"),
                "category": category,
                "name": saved,
            }))
        }
        Err(e) => reply::storage(e),
    }
}

#[delete("/exchange/items/{id}")]
async fn api_delete(
    id: web::Path<String>,
    user: web::ReqData<ApiUser>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    let result = api_file(&config, &store, &user, &id)
        .and_then(|(owner, _, name, path)| Ok((owner, name, fs::remove_file(&path)?)));
    match result {
        Ok((owner, name, ())) => {
            tracing::info!(user = %user.username, exchange = %owner, file = %name, "exchange file deleted (Mini App)");
            reply::ok(json!({ "message": "Удалено из обмена" }))
        }
        Err(e) => reply::storage(e),
    }
}

/// Destination of a finished chunked upload into an exchange (Mini App):
/// rights are checked again, the staged file is published without overwriting.
pub fn finish_upload(
    config: &AppConfig,
    store: &UserStore,
    events: &Events,
    user: &ApiUser,
    owner_name: &str,
    staged: &Path,
    name: &str,
) -> Result<Value, StorageError> {
    let viewer = Viewer::from(user);
    let owner = owner(store, viewer, owner_name)?;
    let dir = outgoing_dir(config, &owner, viewer)?;
    let target = crate::storage::publish_file(staged, &dir, name)?;
    let meta = fs::metadata(&target)?;
    let entry = DirEntry {
        name: target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(name)
            .to_string(),
        is_dir: false,
        size: meta.len(),
        mtime: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs()),
    };
    events.push(&owner, viewer.outgoing(), &entry.name, entry.size);
    Ok(api_item(&owner, viewer.outgoing(), viewer, &entry))
}

/// Check that a chunked upload may go into `owner_name`'s exchange.
pub fn check_upload_target(
    store: &UserStore,
    user: &ApiUser,
    owner_name: &str,
) -> Result<String, StorageError> {
    owner(store, Viewer::from(user), owner_name)
}
