//! JSON replies shared by the Mini App API and the internal API.
//!
//! Same shape as the existing web UI responses: `{"success": bool,
//! "message": …}`, plus a machine-readable `code` on errors the clients react
//! to (`not_linked`, `not_admin`, `conflict`, …).

use actix_web::http::StatusCode;
use actix_web::HttpResponse;
use serde_json::{json, Value};

use crate::storage::StorageError;

/// 200 with `success: true` merged into the given object.
pub fn ok(mut body: Value) -> HttpResponse {
    if let Value::Object(map) = &mut body {
        map.insert("success".into(), Value::Bool(true));
    }
    HttpResponse::Ok().json(body)
}

pub fn error(status: StatusCode, code: &str, message: impl Into<String>) -> HttpResponse {
    HttpResponse::build(status).json(json!({
        "success": false,
        "code": code,
        "message": message.into(),
    }))
}

/// Error with extra fields (e.g. `offset`); `success: false` is added.
pub fn error_with(status: StatusCode, mut body: Value) -> HttpResponse {
    if let Value::Object(map) = &mut body {
        map.insert("success".into(), Value::Bool(false));
    }
    HttpResponse::build(status).json(body)
}

pub fn bad_request(message: impl Into<String>) -> HttpResponse {
    error(StatusCode::BAD_REQUEST, "bad_request", message)
}

pub fn storage(e: StorageError) -> HttpResponse {
    if let StorageError::Io(io) = &e {
        tracing::warn!(error = %io, "storage error");
    }
    error(e.status(), e.code(), e.message())
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
