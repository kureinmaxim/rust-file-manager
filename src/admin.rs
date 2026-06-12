use actix_session::Session;
use actix_web::{delete, post, web, HttpRequest, HttpResponse, Responder};
use serde::Serialize;

use crate::auth::current_user;
use crate::config::AppConfig;
use crate::files::HOME_DIR;
use crate::users::UserStore;

#[derive(Serialize)]
struct InviteResponse {
    success: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    invite_url: Option<String>,
}

/// Create a single-use invite link. The absolute URL is built from the
/// request's connection info (honours X-Forwarded-Proto/Host set by nginx).
#[post("/invites")]
pub async fn create_invite(
    req: HttpRequest,
    session: Session,
    store: web::Data<UserStore>,
) -> impl Responder {
    let username = current_user(&session).map(|u| u.username).unwrap_or_default();
    match store.create_invite(&username) {
        Ok(invite) => {
            let info = req.connection_info();
            let url = format!("{}://{}/register?token={}", info.scheme(), info.host(), invite.token);
            tracing::info!(created_by = %username, "invite link created");
            HttpResponse::Ok().json(InviteResponse {
                success: true,
                message: "Ссылка-приглашение создана (действует 7 дней, на одну регистрацию)".into(),
                invite_url: Some(url),
            })
        }
        Err(e) => HttpResponse::InternalServerError().json(InviteResponse {
            success: false,
            message: e,
            invite_url: None,
        }),
    }
}

#[derive(Serialize)]
struct AdminResponse {
    success: bool,
    message: String,
}

/// Delete a user account together with their private zone.
#[delete("/users/{username}")]
pub async fn delete_user(
    path: web::Path<String>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> impl Responder {
    let username = path.into_inner();
    if let Err(e) = store.remove_user(&username) {
        return HttpResponse::NotFound().json(AdminResponse { success: false, message: e });
    }

    // The username was validated at registration, so this path stays inside home/.
    let home = config.upload_dir.join(HOME_DIR).join(&username);
    if home.is_dir() {
        if let Err(e) = std::fs::remove_dir_all(&home) {
            tracing::warn!(user = %username, error = %e, "failed to remove user home dir");
        }
    }

    tracing::info!(user = %username, "user deleted");
    HttpResponse::Ok().json(AdminResponse {
        success: true,
        message: format!("Пользователь «{username}» и его файлы удалены"),
    })
}
