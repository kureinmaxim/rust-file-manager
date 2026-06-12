use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use actix_multipart::Multipart;
use actix_web::{delete, get, post, web, Error, HttpResponse};
use futures_util::StreamExt;
use handlebars::Handlebars;
use serde::Serialize;
use serde_json::json;

use crate::categories::{category_for_extension, is_valid_category, sanitize_file_name, FILE_CATEGORIES};
use crate::config::AppConfig;

#[derive(Serialize)]
struct CategoryFiles {
    category: String,
    files: Vec<String>,
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

/// Resolve `<upload_dir>/<category>/<file>`, rejecting invalid categories and
/// unsafe file names. Both segments are validated, so the result cannot
/// escape the upload directory.
fn safe_path(config: &AppConfig, category: &str, file_name: &str) -> Result<PathBuf, HttpResponse> {
    if !is_valid_category(category) {
        return Err(bad_request("Недопустимая категория"));
    }
    let name = sanitize_file_name(file_name).ok_or_else(|| bad_request("Недопустимое имя файла"))?;
    Ok(config.upload_dir.join(category).join(name))
}

#[get("/")]
pub async fn index(
    config: web::Data<AppConfig>,
    hb: web::Data<Handlebars<'static>>,
) -> Result<HttpResponse, Error> {
    let mut categories = Vec::new();
    for (category, _) in FILE_CATEGORIES {
        let dir = config.upload_dir.join(category);
        let Ok(entries) = fs::read_dir(&dir) else { continue };

        let mut files: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        files.sort();

        if !files.is_empty() {
            categories.push(CategoryFiles { category: category.to_string(), files });
        }
    }

    let html = hb
        .render(
            "index",
            &json!({
                "categories": categories,
                "total_size": format_bytes(folder_size(&config.upload_dir)),
                "max_file_size": format_bytes(config.max_file_size as u64),
                "version": env!("CARGO_PKG_VERSION"),
                "git_commit": env!("BUILD_GIT_COMMIT"),
                "build_date": env!("BUILD_DATE"),
            }),
        )
        .map_err(actix_web::error::ErrorInternalServerError)?;

    Ok(HttpResponse::Ok().content_type("text/html; charset=utf-8").body(html))
}

#[post("/upload")]
pub async fn upload(
    mut payload: Multipart,
    config: web::Data<AppConfig>,
) -> Result<HttpResponse, Error> {
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
        let category = category_for_extension(extension);
        let category_dir = config.upload_dir.join(category);
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
        tracing::info!(category, file = saved_name, size, "file uploaded");
        message = format!("Файл загружен в категорию «{category}»: {saved_name}");
    }

    Ok(ApiResponse::ok(message, Some(format_bytes(folder_size(&config.upload_dir)))))
}

#[delete("/delete/{category}/{filename}")]
pub async fn delete_file(
    path: web::Path<(String, String)>,
    config: web::Data<AppConfig>,
) -> Result<HttpResponse, Error> {
    let (category, file_name) = path.into_inner();
    let target = match safe_path(&config, &category, &file_name) {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };

    if !target.is_file() {
        return Ok(not_found("Файл не найден"));
    }
    fs::remove_file(&target)?;
    tracing::info!(category = %category, file = %file_name, "file deleted");

    Ok(ApiResponse::ok(
        format!("Файл удалён: {category}/{file_name}"),
        Some(format_bytes(folder_size(&config.upload_dir))),
    ))
}

#[derive(serde::Deserialize)]
pub struct RenameQuery {
    #[serde(rename = "newName")]
    new_name: String,
}

#[post("/rename/{category}/{filename}")]
pub async fn rename_file(
    path: web::Path<(String, String)>,
    query: web::Query<RenameQuery>,
    config: web::Data<AppConfig>,
) -> Result<HttpResponse, Error> {
    let (category, file_name) = path.into_inner();
    let old_path = match safe_path(&config, &category, &file_name) {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };
    let new_path = match safe_path(&config, &category, &query.new_name) {
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
    tracing::info!(category = %category, from = %file_name, to = %query.new_name, "file renamed");

    Ok(ApiResponse::ok(
        format!("Файл переименован: {} → {}", file_name, query.new_name),
        None,
    ))
}
