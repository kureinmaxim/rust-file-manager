use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use actix_files::NamedFile;
use actix_multipart::Multipart;
use actix_session::Session;
use actix_web::{delete, get, post, web, Error, HttpRequest, HttpResponse};
use futures_util::StreamExt;
use handlebars::Handlebars;
use serde::Serialize;
use serde_json::json;

use crate::auth::{current_user, CurrentUser};
use crate::categories::{
    category_for_extension, category_rel_dir, sanitize_file_name, BACKUP_FOLDERS, BACKUP_PARENT,
    FILE_CATEGORIES,
};
use crate::config::AppConfig;

pub const SHARED_DIR: &str = "shared";
pub const HOME_DIR: &str = "home";

#[derive(Serialize)]
struct CategoryFiles {
    category: String,
    title: String,
    files: Vec<String>,
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
        HttpResponse::Ok().json(ApiResponse { success: true, message, total_size })
    }

    fn err(status: actix_web::http::StatusCode, message: String) -> HttpResponse {
        HttpResponse::build(status).json(ApiResponse { success: false, message, total_size: None })
    }
}

fn bad_request(message: impl Into<String>) -> HttpResponse {
    ApiResponse::err(actix_web::http::StatusCode::BAD_REQUEST, message.into())
}

fn not_found(message: impl Into<String>) -> HttpResponse {
    ApiResponse::err(actix_web::http::StatusCode::NOT_FOUND, message.into())
}

fn unauthorized() -> HttpResponse {
    ApiResponse::err(actix_web::http::StatusCode::UNAUTHORIZED, "Требуется вход".into())
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let i = ((bytes as f64).log(1024.0).floor() as usize).min(units.len() - 1);
    format!("{:.2} {}", bytes as f64 / 1024f64.powi(i as i32), units[i])
}

fn folder_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|entry| {
            let p = entry.path();
            if p.is_dir() {
                folder_size(&p)
            } else {
                fs::metadata(&p).map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
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
    let rel_dir = category_rel_dir(category).ok_or_else(|| bad_request("Недопустимая категория"))?;
    let name = sanitize_file_name(file_name).ok_or_else(|| bad_request("Недопустимое имя файла"))?;
    Ok(root.join(rel_dir).join(name))
}

/// Category ids paired with their UI titles: regular categories first, then
/// the backup folders shown as «Бэкапы — <folder>».
fn category_listing() -> Vec<(String, String)> {
    FILE_CATEGORIES
        .iter()
        .map(|(c, _)| (c.to_string(), c.to_string()))
        .chain(
            BACKUP_FOLDERS
                .iter()
                .map(|f| (f.to_string(), format!("💾 {BACKUP_PARENT} — {f}"))),
        )
        .collect()
}

fn list_zone(root: &Path) -> Vec<CategoryFiles> {
    let mut categories = Vec::new();
    for (category, title) in category_listing() {
        let rel_dir = category_rel_dir(&category).expect("listing only yields valid categories");
        let dir = root.join(rel_dir);
        let Ok(entries) = fs::read_dir(&dir) else { continue };

        let mut files: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        files.sort();

        if !files.is_empty() {
            categories.push(CategoryFiles { category, title, files });
        }
    }
    categories
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
            categories: list_zone(&my_root),
            total_size: format_bytes(folder_size(&my_root)),
        },
        ZoneView {
            scope: "shared".into(),
            title: "Общие файлы".into(),
            icon: "👥".into(),
            categories: list_zone(&shared_root),
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

    let html = hb
        .render(
            "index",
            &json!({
                "username": user.username,
                "is_admin": user.is_admin,
                "zones": zones,
                "users": users,
                "max_file_size": format_bytes(config.max_file_size as u64),
                "version": env!("CARGO_PKG_VERSION"),
                "git_commit": env!("BUILD_GIT_COMMIT"),
                "build_date": env!("BUILD_DATE"),
            }),
        )
        .map_err(actix_web::error::ErrorInternalServerError)?;

    Ok(HttpResponse::Ok().content_type("text/html; charset=utf-8").body(html))
}

/// Authenticated download. Replaces a blanket static-files mount: the zone is
/// resolved against the session, so users can only ever read `shared/` and
/// their own `home/<username>/`.
#[get("/uploads/{scope}/{category}/{filename}")]
pub async fn download(
    req: HttpRequest,
    path: web::Path<(String, String, String)>,
    session: Session,
    config: web::Data<AppConfig>,
) -> Result<HttpResponse, Error> {
    let Some(user) = current_user(&session) else {
        return Ok(unauthorized());
    };
    let (scope, category, file_name) = path.into_inner();
    let target = match safe_path(&config, &scope, &user, &category, &file_name) {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };
    if !target.is_file() {
        return Ok(not_found("Файл не найден"));
    }
    Ok(NamedFile::open(target)?
        .use_last_modified(true)
        .into_response(&req))
}

#[derive(serde::Deserialize)]
pub struct UploadQuery {
    /// Explicit target category (e.g. a backup folder). When absent the
    /// category is derived from the file extension.
    category: Option<String>,
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

    let mut message = String::from("Файл не получен");

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
        let rel_dir = category_rel_dir(&category).expect("category validated above");
        let category_dir = root.join(rel_dir);
        fs::create_dir_all(&category_dir)?;

        // Avoid overwriting: append (1), (2), ... until the name is free.
        let stem = Path::new(&file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let mut target = category_dir.join(&file_name);
        let mut counter = 1;
        while target.exists() {
            let suffixed = if extension.is_empty() {
                format!("{stem}({counter})")
            } else {
                format!("{stem}({counter}).{extension}")
            };
            target = category_dir.join(suffixed);
            counter += 1;
        }

        let mut file = fs::File::create(&target)?;
        let mut size = 0usize;
        while let Some(chunk) = field.next().await {
            let data = chunk?;
            size += data.len();
            if size > config.max_file_size {
                drop(file);
                let _ = fs::remove_file(&target);
                return Ok(ApiResponse::err(
                    actix_web::http::StatusCode::PAYLOAD_TOO_LARGE,
                    format!("Файл превышает лимит {}", format_bytes(config.max_file_size as u64)),
                ));
            }
            file.write_all(&data)?;
        }

        let saved_name = target.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        tracing::info!(user = %user.username, scope = %scope.as_str(), category = %category, file = saved_name, size, "file uploaded");
        message = format!("Файл загружен в категорию «{category}»: {saved_name}");
    }

    Ok(ApiResponse::ok(message, Some(format_bytes(folder_size(&root)))))
}

#[delete("/delete/{scope}/{category}/{filename}")]
pub async fn delete_file(
    path: web::Path<(String, String, String)>,
    session: Session,
    config: web::Data<AppConfig>,
) -> Result<HttpResponse, Error> {
    let Some(user) = current_user(&session) else {
        return Ok(unauthorized());
    };
    let (scope, category, file_name) = path.into_inner();
    let target = match safe_path(&config, &scope, &user, &category, &file_name) {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };

    if !target.is_file() {
        return Ok(not_found("Файл не найден"));
    }
    fs::remove_file(&target)?;
    tracing::info!(user = %user.username, scope = %scope, category = %category, file = %file_name, "file deleted");

    Ok(ApiResponse::ok(
        format!("Файл удалён: {category}/{file_name}"),
        None,
    ))
}

#[derive(serde::Deserialize)]
pub struct RenameQuery {
    #[serde(rename = "newName")]
    new_name: String,
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
    let old_path = match safe_path(&config, &scope, &user, &category, &file_name) {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };
    let new_path = match safe_path(&config, &scope, &user, &category, &query.new_name) {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };

    if !old_path.is_file() {
        return Ok(not_found("Файл не найден"));
    }
    if new_path.exists() {
        return Ok(bad_request("Файл с таким именем уже существует"));
    }

    fs::rename(&old_path, &new_path)?;
    tracing::info!(user = %user.username, scope = %scope, category = %category, from = %file_name, to = %query.new_name, "file renamed");

    Ok(ApiResponse::ok(
        format!("Файл переименован: {} → {}", file_name, query.new_name),
        None,
    ))
}
