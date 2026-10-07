use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use actix_files::NamedFile;
use actix_multipart::Multipart;
use actix_session::Session;
use actix_web::http::header::{ContentDisposition, DispositionParam, DispositionType};
use actix_web::{delete, get, post, web, Error, HttpRequest, HttpResponse};
use futures_util::StreamExt;
use handlebars::Handlebars;
use serde::Serialize;
use serde_json::json;

use crate::auth::{current_user, CurrentUser};
use crate::categories::{category_for_extension, category_rel_dir, sanitize_file_name};
use crate::config::AppConfig;
use crate::paths::RelPath;
use crate::storage::{
    self, category_listing, encode_url_segment, exact_file_name, folder_dir, folder_size,
    format_bytes, read_dir_entries, PendingUpload, StorageError, Zone, HOME_DIR, SHARED_DIR,
    WALK_LIMIT,
};

#[derive(Serialize)]
struct CategoryFiles {
    scope: String,
    category: String,
    title: String,
    files: Vec<FileView>,
    folders: Vec<FolderView>,
}

#[derive(Serialize)]
struct FileView {
    scope: String,
    category: String,
    /// Folder inside the category, `a/b`; empty for the category root.
    path: String,
    name: String,
    url: String,
    size: String,
    size_bytes: u64,
    modified: Option<u64>,
}

/// Subfolder of a category (shared with the Telegram Mini App), rendered as a
/// nested tree by the `folder` partial.
#[derive(Serialize)]
struct FolderView {
    scope: String,
    category: String,
    path: String,
    name: String,
    files: Vec<FileView>,
    folders: Vec<FolderView>,
}

#[derive(Serialize)]
struct ZoneView {
    scope: String,
    title: String,
    icon: String,
    categories: Vec<CategoryFiles>,
    total_size: String,
}

#[derive(Serialize)]
struct ApiResponse {
    success: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_size: Option<String>,
}

impl ApiResponse {
    fn ok(message: String, total_size: Option<String>) -> HttpResponse {
        HttpResponse::Ok().json(ApiResponse {
            success: true,
            message,
            total_size,
        })
    }

    fn err(status: actix_web::http::StatusCode, message: String) -> HttpResponse {
        HttpResponse::build(status).json(ApiResponse {
            success: false,
            message,
            total_size: None,
        })
    }
}

fn bad_request(message: impl Into<String>) -> HttpResponse {
    ApiResponse::err(actix_web::http::StatusCode::BAD_REQUEST, message.into())
}

fn not_found(message: impl Into<String>) -> HttpResponse {
    ApiResponse::err(actix_web::http::StatusCode::NOT_FOUND, message.into())
}

fn unauthorized() -> HttpResponse {
    ApiResponse::err(
        actix_web::http::StatusCode::UNAUTHORIZED,
        "Требуется вход".into(),
    )
}

fn storage_error(e: StorageError) -> HttpResponse {
    ApiResponse::err(e.status(), e.message())
}

/// Root directory of a zone: `shared/` is visible to everyone, `my` maps to
/// the per-user `home/<username>/` directory nobody else can reach — the
/// username comes from the session, never from the URL.
fn zone_root(config: &AppConfig, scope: &str, user: &CurrentUser) -> Result<PathBuf, HttpResponse> {
    match scope {
        "shared" => Ok(config.upload_dir.join(SHARED_DIR)),
        "my" => Ok(config.upload_dir.join(HOME_DIR).join(&user.username)),
        _ => Err(bad_request("Недопустимая зона")),
    }
}

/// Resolve `<zone>/<category>/<file>`, rejecting invalid categories and
/// unsafe file names. Every segment is validated, so the result cannot
/// escape the zone directory.
fn safe_path(
    config: &AppConfig,
    scope: &str,
    user: &CurrentUser,
    category: &str,
    file_name: &str,
) -> Result<PathBuf, HttpResponse> {
    let root = zone_root(config, scope, user)?;
    let rel_dir =
        category_rel_dir(category).ok_or_else(|| bad_request("Недопустимая категория"))?;
    let name =
        sanitize_file_name(file_name).ok_or_else(|| bad_request("Недопустимое имя файла"))?;
    Ok(root.join(rel_dir).join(name))
}

/// `?path=a/b` selects a subfolder of the category. Without it the routes
/// behave exactly as before subfolders existed.
#[derive(serde::Deserialize)]
pub struct FolderQuery {
    #[serde(default)]
    path: String,
}

fn parse_folder(path: &str) -> Result<RelPath, HttpResponse> {
    RelPath::parse(path).map_err(bad_request)
}

fn parse_zone(scope: &str) -> Result<Zone, HttpResponse> {
    Zone::parse(scope).ok_or_else(|| bad_request("Недопустимая зона"))
}

/// A file in the category root (legacy rules) or in a subfolder: there the
/// name must be exact and no component may be a symlink, as in the Mini App.
fn target_path(
    config: &AppConfig,
    scope: &str,
    user: &CurrentUser,
    category: &str,
    folder: &RelPath,
    file_name: &str,
) -> Result<PathBuf, HttpResponse> {
    if folder.is_root() {
        return safe_path(config, scope, user, category, file_name);
    }
    let zone = parse_zone(scope)?;
    storage::file_path(config, zone, &user.username, category, folder, file_name)
        .map_err(storage_error)
}

fn file_url(scope: &str, category: &str, folder: &str, name: &str) -> String {
    let mut url = format!(
        "/uploads/{}/{}/{}",
        encode_url_segment(scope),
        encode_url_segment(category),
        encode_url_segment(name),
    );
    if !folder.is_empty() {
        url.push_str("?path=");
        url.push_str(&encode_url_segment(folder));
    }
    url
}

/// Where a listing stands: which zone and category it belongs to, and how
/// many entries may still be read (a bound against huge trees).
struct ListContext<'a> {
    scope: &'a str,
    category: &'a str,
    budget: usize,
}

/// Files and subfolders of `dir`, recursively; symlinks are skipped and
/// folders whose names the path rules reject (e.g. hidden ones) are not shown.
fn read_folder(
    ctx: &mut ListContext,
    dir: &Path,
    rel: &RelPath,
) -> (Vec<FileView>, Vec<FolderView>) {
    let mut files = Vec::new();
    let mut folders = Vec::new();
    let Ok(entries) = read_dir_entries(dir) else {
        return (files, folders);
    };
    let path = rel.to_string();
    for entry in entries {
        if ctx.budget == 0 {
            break;
        }
        ctx.budget -= 1;
        if entry.is_dir {
            let Ok(child) = rel.join(&entry.name) else {
                continue;
            };
            let (child_files, child_folders) = read_folder(ctx, &dir.join(&entry.name), &child);
            folders.push(FolderView {
                scope: ctx.scope.to_string(),
                category: ctx.category.to_string(),
                path: child.to_string(),
                name: entry.name,
                files: child_files,
                folders: child_folders,
            });
        } else {
            // Subfolder files are reached by exact name only; the root keeps
            // the legacy lenient rules.
            if !rel.is_root() && exact_file_name(&entry.name).is_err() {
                continue;
            }
            files.push(FileView {
                scope: ctx.scope.to_string(),
                category: ctx.category.to_string(),
                url: file_url(ctx.scope, ctx.category, &path, &entry.name),
                path: path.clone(),
                size: format_bytes(entry.size),
                size_bytes: entry.size,
                modified: entry.mtime,
                name: entry.name,
            });
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    folders.sort_by_key(|f| f.name.to_lowercase());
    (files, folders)
}

fn list_zone(root: &Path, scope: &str) -> Vec<CategoryFiles> {
    let mut categories = Vec::new();
    let mut budget = WALK_LIMIT;
    for (category, title) in category_listing() {
        let rel_dir = category_rel_dir(&category).expect("listing only yields valid categories");
        let mut ctx = ListContext {
            scope,
            category: &category,
            budget,
        };
        let (files, folders) = read_folder(&mut ctx, &root.join(rel_dir), &RelPath::root());
        budget = ctx.budget;
        if !files.is_empty() || !folders.is_empty() {
            categories.push(CategoryFiles {
                scope: scope.to_string(),
                category,
                title,
                files,
                folders,
            });
        }
    }
    categories
}

/// The web UI templates: the page and the partials of its file tree
/// (`folder` renders itself for nested folders).
pub fn templates() -> Handlebars<'static> {
    let mut handlebars = Handlebars::new();
    handlebars
        .register_template_string("index", include_str!("../templates/index.html"))
        .expect("invalid index template");
    handlebars
        .register_partial("file_row", include_str!("../templates/_file_row.html"))
        .expect("invalid file_row partial");
    handlebars
        .register_partial("folder", include_str!("../templates/_folder.html"))
        .expect("invalid folder partial");
    handlebars
}

#[get("/")]
pub async fn index(
    session: Session,
    config: web::Data<AppConfig>,
    store: web::Data<crate::users::UserStore>,
    hb: web::Data<Handlebars<'static>>,
) -> Result<HttpResponse, Error> {
    let Some(user) = current_user(&session) else {
        return Ok(unauthorized());
    };

    let my_root = config.upload_dir.join(HOME_DIR).join(&user.username);
    let shared_root = config.upload_dir.join(SHARED_DIR);

    let zones = vec![
        ZoneView {
            scope: "my".into(),
            title: "Мои файлы".into(),
            icon: "🔒".into(),
            categories: list_zone(&my_root, "my"),
            total_size: format_bytes(folder_size(&my_root)),
        },
        ZoneView {
            scope: "shared".into(),
            title: "Общие файлы".into(),
            icon: "👥".into(),
            categories: list_zone(&shared_root, "shared"),
            total_size: format_bytes(folder_size(&shared_root)),
        },
    ];

    let users: Vec<serde_json::Value> = if user.is_admin {
        store
            .list_users()
            .iter()
            .map(|u| json!({ "username": u.username }))
            .collect()
    } else {
        Vec::new()
    };
    let all_categories: Vec<serde_json::Value> = category_listing()
        .into_iter()
        .map(|(id, title)| json!({ "id": id, "title": title }))
        .collect();

    let html = hb
        .render(
            "index",
            &json!({
                "username": user.username,
                "is_admin": user.is_admin,
                "zones": zones,
                "users": users,
                "categories": all_categories,
                "max_file_size": format_bytes(config.max_file_size as u64),
                "max_file_size_bytes": config.max_file_size,
                "version": env!("CARGO_PKG_VERSION"),
                "git_commit": env!("BUILD_GIT_COMMIT"),
                "build_date": env!("BUILD_DATE"),
            }),
        )
        .map_err(actix_web::error::ErrorInternalServerError)?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html))
}

/// Authenticated download. Replaces a blanket static-files mount: the zone is
/// resolved against the session, so users can only ever read `shared/` and
/// their own `home/<username>/`.
#[get("/uploads/{scope}/{category}/{filename}")]
pub async fn download(
    req: HttpRequest,
    path: web::Path<(String, String, String)>,
    query: web::Query<FolderQuery>,
    session: Session,
    config: web::Data<AppConfig>,
) -> Result<HttpResponse, Error> {
    let Some(user) = current_user(&session) else {
        return Ok(unauthorized());
    };
    let (scope, category, file_name) = path.into_inner();
    let target = match parse_folder(&query.path)
        .and_then(|folder| target_path(&config, &scope, &user, &category, &folder, &file_name))
    {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };
    if !target.is_file() {
        return Ok(not_found("Файл не найден"));
    }
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("download")
        .to_string();
    // User-controlled HTML/SVG must not execute under the application's origin.
    Ok(NamedFile::open(target)?
        .set_content_disposition(ContentDisposition {
            disposition: DispositionType::Attachment,
            parameters: vec![DispositionParam::Filename(name)],
        })
        .use_last_modified(true)
        .into_response(&req))
}

#[derive(serde::Deserialize)]
pub struct UploadQuery {
    /// Explicit target category (e.g. a backup folder). When absent the
    /// category is derived from the file extension.
    category: Option<String>,
    /// Existing subfolder of that category; requires `category`.
    #[serde(default)]
    path: String,
}

#[post("/upload/{scope}")]
pub async fn upload(
    scope: web::Path<String>,
    query: web::Query<UploadQuery>,
    mut payload: Multipart,
    session: Session,
    config: web::Data<AppConfig>,
) -> Result<HttpResponse, Error> {
    let Some(user) = current_user(&session) else {
        return Ok(unauthorized());
    };
    let root = match zone_root(&config, &scope, &user) {
        Ok(r) => r,
        Err(resp) => return Ok(resp),
    };
    let forced_category = match query.category.as_deref().filter(|c| !c.is_empty()) {
        Some(c) => match category_rel_dir(c) {
            Some(_) => Some(c.to_string()),
            None => return Ok(bad_request("Недопустимая категория")),
        },
        None => None,
    };
    let folder = match parse_folder(&query.path) {
        Ok(f) => f,
        Err(resp) => return Ok(resp),
    };
    // A subfolder belongs to one category, so it cannot be combined with
    // choosing the category by extension. It must already exist.
    let folder_target = if folder.is_root() {
        None
    } else {
        let Some(category) = forced_category.as_deref() else {
            return Ok(bad_request("Для загрузки в папку укажите категорию"));
        };
        let dir = match parse_zone(&scope).and_then(|zone| {
            folder_dir(&config, zone, &user.username, category, &folder).map_err(storage_error)
        }) {
            Ok(d) => d,
            Err(resp) => return Ok(resp),
        };
        if !dir.is_dir() {
            return Ok(not_found("Папка не найдена"));
        }
        Some(dir)
    };

    let mut message = None;

    while let Some(item) = payload.next().await {
        let mut field = item?;

        let raw_name = field
            .content_disposition()
            .and_then(|cd| cd.get_filename())
            .unwrap_or("")
            .to_string();
        let Some(file_name) = sanitize_file_name(&raw_name) else {
            return Ok(bad_request("Недопустимое имя файла"));
        };

        let extension = Path::new(&file_name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let category = match &forced_category {
            Some(c) => c.clone(),
            None => category_for_extension(extension).to_string(),
        };
        let target_dir = match &folder_target {
            Some(dir) => dir.clone(),
            None => {
                let rel_dir = category_rel_dir(&category).expect("category validated above");
                let category_dir = root.join(rel_dir);
                fs::create_dir_all(&category_dir)?;
                category_dir
            }
        };

        // Reserve the name atomically, including across simultaneous uploads.
        let mut pending = PendingUpload::create(&target_dir, &file_name)?;
        let mut size = 0usize;
        while let Some(chunk) = field.next().await {
            let data = chunk?;
            size += data.len();
            if size > config.max_file_size {
                return Ok(ApiResponse::err(
                    actix_web::http::StatusCode::PAYLOAD_TOO_LARGE,
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
        let saved_name = pending
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");
        tracing::info!(user = %user.username, scope = %scope.as_str(), category = %category, folder = %folder, file = saved_name, size, "file uploaded");
        message = Some(if folder.is_root() {
            format!("Файл загружен в категорию «{category}»: {saved_name}")
        } else {
            format!("Файл загружен в папку «{category} / {folder}»: {saved_name}")
        });
    }

    match message {
        Some(message) => Ok(ApiResponse::ok(
            message,
            Some(format_bytes(folder_size(&root))),
        )),
        None => Ok(bad_request("Файл не получен")),
    }
}

#[delete("/delete/{scope}/{category}/{filename}")]
pub async fn delete_file(
    path: web::Path<(String, String, String)>,
    query: web::Query<FolderQuery>,
    session: Session,
    config: web::Data<AppConfig>,
) -> Result<HttpResponse, Error> {
    let Some(user) = current_user(&session) else {
        return Ok(unauthorized());
    };
    let (scope, category, file_name) = path.into_inner();
    let target = match parse_folder(&query.path)
        .and_then(|folder| target_path(&config, &scope, &user, &category, &folder, &file_name))
    {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };

    if !target.is_file() {
        return Ok(not_found("Файл не найден"));
    }
    fs::remove_file(&target)?;
    tracing::info!(user = %user.username, scope = %scope, category = %category, folder = %query.path, file = %file_name, "file deleted");

    let shown = if query.path.is_empty() {
        format!("{category}/{file_name}")
    } else {
        format!("{category}/{}/{file_name}", query.path)
    };
    Ok(ApiResponse::ok(format!("Файл удалён: {shown}"), None))
}

#[derive(serde::Deserialize)]
pub struct RenameQuery {
    #[serde(rename = "newName")]
    new_name: String,
    #[serde(default)]
    path: String,
}

#[post("/rename/{scope}/{category}/{filename}")]
pub async fn rename_file(
    path: web::Path<(String, String, String)>,
    query: web::Query<RenameQuery>,
    session: Session,
    config: web::Data<AppConfig>,
) -> Result<HttpResponse, Error> {
    let Some(user) = current_user(&session) else {
        return Ok(unauthorized());
    };
    let (scope, category, file_name) = path.into_inner();
    let folder = match parse_folder(&query.path) {
        Ok(f) => f,
        Err(resp) => return Ok(resp),
    };
    let old_path = match target_path(&config, &scope, &user, &category, &folder, &file_name) {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };
    let new_path = match target_path(&config, &scope, &user, &category, &folder, &query.new_name) {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };

    if !old_path.is_file() {
        return Ok(not_found("Файл не найден"));
    }
    if folder.is_root() {
        if new_path.exists() {
            return Ok(bad_request("Файл с таким именем уже существует"));
        }
        fs::rename(&old_path, &new_path)?;
    } else {
        // Hard link + remove, as in the Mini App: fails instead of overwriting.
        if let Err(e) = fs::hard_link(&old_path, &new_path) {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                return Ok(bad_request("Файл с таким именем уже существует"));
            }
            return Err(e.into());
        }
        fs::remove_file(&old_path)?;
    }
    tracing::info!(user = %user.username, scope = %scope, category = %category, folder = %folder, from = %file_name, to = %query.new_name, "file renamed");

    Ok(ApiResponse::ok(
        format!("Файл переименован: {} → {}", file_name, query.new_name),
        None,
    ))
}

#[derive(serde::Deserialize)]
pub struct MoveQuery {
    /// Folder of the file now; empty = the category root.
    #[serde(default)]
    path: String,
    #[serde(rename = "toScope")]
    to_scope: String,
    #[serde(rename = "toCategory")]
    to_category: String,
    /// Existing folder of the destination; empty = its category root.
    #[serde(rename = "toPath", default)]
    to_path: String,
}

fn zone_title(zone: Zone) -> &'static str {
    match zone {
        Zone::My => "Мои файлы",
        Zone::Shared => "Общие файлы",
    }
}

/// Move a file between zones (shared ↔ my), categories and folders. Like
/// uploads it never overwrites: a taken name gets a `(1)` suffix. Anyone who
/// can see a shared file may already delete it, so taking it into the own
/// zone grants no new power.
#[post("/move/{scope}/{category}/{filename}")]
pub async fn move_file(
    path: web::Path<(String, String, String)>,
    query: web::Query<MoveQuery>,
    session: Session,
    config: web::Data<AppConfig>,
) -> Result<HttpResponse, Error> {
    let Some(user) = current_user(&session) else {
        return Ok(unauthorized());
    };
    let (scope, category, file_name) = path.into_inner();
    let source = match parse_folder(&query.path)
        .and_then(|folder| target_path(&config, &scope, &user, &category, &folder, &file_name))
    {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };
    if !source.is_file() {
        return Ok(not_found("Файл не найден"));
    }
    let (to_zone, to_folder) = match parse_zone(&query.to_scope)
        .and_then(|zone| parse_folder(&query.to_path).map(|folder| (zone, folder)))
    {
        Ok(pair) => pair,
        Err(resp) => return Ok(resp),
    };
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    // Folder files are reached by exact name only (see read_folder).
    if !to_folder.is_root() && exact_file_name(&name).is_err() {
        return Ok(bad_request(
            "Переименуйте файл перед переносом в папку: в имени лишние пробелы",
        ));
    }
    // The category root is created on demand, as for uploads; a folder must exist.
    let dest_dir = match folder_dir(
        &config,
        to_zone,
        &user.username,
        &query.to_category,
        &to_folder,
    ) {
        Ok(dir) => dir,
        Err(e) => return Ok(storage_error(e)),
    };
    if to_folder.is_root() {
        fs::create_dir_all(&dest_dir)?;
    } else if !dest_dir.is_dir() {
        return Ok(not_found("Папка назначения не найдена"));
    }
    if source.parent() == Some(dest_dir.as_path()) {
        return Ok(bad_request("Файл уже находится в этой папке"));
    }
    let moved = match storage::publish_file(&source, &dest_dir, &name) {
        Ok(p) => p,
        Err(e) => return Ok(storage_error(e.into())),
    };
    let final_name = moved.file_name().and_then(|n| n.to_str()).unwrap_or(&name);
    let place = [zone_title(to_zone), query.to_category.as_str()]
        .into_iter()
        .chain(to_folder.segments().iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" › ");
    tracing::info!(user = %user.username, from_scope = %scope, from_category = %category, from_folder = %query.path, to_scope = to_zone.as_str(), to_category = %query.to_category, to_folder = %to_folder, file = final_name, "file moved");
    Ok(ApiResponse::ok(
        format!("Файл перемещён в «{place}»: {final_name}"),
        None,
    ))
}

#[derive(serde::Deserialize)]
pub struct NewFolderQuery {
    /// Parent folder; empty = the category root.
    #[serde(default)]
    path: String,
    name: String,
}

/// Same folders as in the Telegram Mini App: one tree per category.
#[post("/folders/{scope}/{category}")]
pub async fn create_folder(
    path: web::Path<(String, String)>,
    query: web::Query<NewFolderQuery>,
    session: Session,
    config: web::Data<AppConfig>,
) -> HttpResponse {
    let Some(user) = current_user(&session) else {
        return unauthorized();
    };
    let (scope, category) = path.into_inner();
    let result = parse_zone(&scope)
        .and_then(|zone| parse_folder(&query.path).map(|parent| (zone, parent)))
        .and_then(|(zone, parent)| {
            storage::create_folder(
                &config,
                zone,
                &user.username,
                &category,
                &parent,
                &query.name,
            )
            .map_err(storage_error)
        });
    match result {
        Ok((folder, _)) => {
            tracing::info!(user = %user.username, scope = %scope, category = %category, folder = %folder, "folder created");
            ApiResponse::ok(format!("Папка создана: {category} / {folder}"), None)
        }
        Err(resp) => resp,
    }
}

#[derive(serde::Deserialize)]
pub struct FolderRenameQuery {
    path: String,
    #[serde(rename = "newName")]
    new_name: String,
}

#[post("/folders/{scope}/{category}/rename")]
pub async fn rename_folder(
    path: web::Path<(String, String)>,
    query: web::Query<FolderRenameQuery>,
    session: Session,
    config: web::Data<AppConfig>,
) -> HttpResponse {
    let Some(user) = current_user(&session) else {
        return unauthorized();
    };
    let (scope, category) = path.into_inner();
    let result = parse_zone(&scope)
        .and_then(|zone| parse_folder(&query.path).map(|folder| (zone, folder)))
        .and_then(|(zone, folder)| {
            storage::rename_folder(
                &config,
                zone,
                &user.username,
                &category,
                &folder,
                &query.new_name,
            )
            .map_err(storage_error)
        });
    match result {
        Ok((folder, _)) => {
            tracing::info!(user = %user.username, scope = %scope, category = %category, from = %query.path, to = %folder, "folder renamed");
            ApiResponse::ok(format!("Папка переименована: {category} / {folder}"), None)
        }
        Err(resp) => resp,
    }
}

/// Only empty folders can be deleted: there is no trash yet.
#[delete("/folders/{scope}/{category}")]
pub async fn delete_folder(
    path: web::Path<(String, String)>,
    query: web::Query<FolderQuery>,
    session: Session,
    config: web::Data<AppConfig>,
) -> HttpResponse {
    let Some(user) = current_user(&session) else {
        return unauthorized();
    };
    let (scope, category) = path.into_inner();
    let result = parse_zone(&scope)
        .and_then(|zone| parse_folder(&query.path).map(|folder| (zone, folder)))
        .and_then(|(zone, folder)| {
            if folder.is_root() {
                return Err(bad_request("Выберите папку"));
            }
            folder_dir(&config, zone, &user.username, &category, &folder)
                .and_then(|dir| storage::remove_empty_folder(&dir))
                .map_err(storage_error)
        });
    match result {
        Ok(()) => {
            tracing::info!(user = %user.username, scope = %scope, category = %category, folder = %query.path, "folder deleted");
            ApiResponse::ok(format!("Папка удалена: {category} / {}", query.path), None)
        }
        Err(resp) => resp,
    }
}
