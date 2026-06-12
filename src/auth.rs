use actix_session::{Session, SessionExt};
use actix_web::body::{BoxBody, MessageBody};
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::http::header;
use actix_web::middleware::Next;
use actix_web::{web, Error, HttpResponse, Responder};
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;

const SESSION_USER_KEY: &str = "user";

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    success: bool,
    message: String,
}

/// Middleware guard: every route behind it requires a logged-in session.
/// Browser navigation gets a redirect to /login, API calls get 401 JSON.
pub async fn require_auth(
    req: ServiceRequest,
    next: Next<impl MessageBody + 'static>,
) -> Result<ServiceResponse<BoxBody>, Error> {
    let logged_in = req
        .get_session()
        .get::<String>(SESSION_USER_KEY)
        .unwrap_or(None)
        .is_some();

    if logged_in {
        return Ok(next.call(req).await?.map_into_boxed_body());
    }

    let wants_html = req
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"));

    let (req, _) = req.into_parts();
    let response = if wants_html {
        HttpResponse::SeeOther()
            .insert_header((header::LOCATION, "/login"))
            .finish()
    } else {
        HttpResponse::Unauthorized().json(LoginResponse {
            success: false,
            message: "Требуется вход".to_string(),
        })
    };
    Ok(ServiceResponse::new(req, response))
}

pub async fn login_page() -> impl Responder {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(include_str!("../templates/login.html"))
}

pub async fn login(
    form: web::Form<LoginForm>,
    session: Session,
    config: web::Data<AppConfig>,
) -> impl Responder {
    let password_ok = form.username == config.admin_username
        && bcrypt::verify(&form.password, &config.admin_password_hash).unwrap_or(false);

    if !password_ok {
        tracing::warn!(username = %form.username, "failed login attempt");
        return HttpResponse::Unauthorized().json(LoginResponse {
            success: false,
            message: "Неверное имя пользователя или пароль".to_string(),
        });
    }

    session.renew();
    if let Err(e) = session.insert(SESSION_USER_KEY, &form.username) {
        return HttpResponse::InternalServerError().json(LoginResponse {
            success: false,
            message: format!("Ошибка сессии: {e}"),
        });
    }

    tracing::info!(username = %form.username, "login successful");
    HttpResponse::Ok().json(LoginResponse {
        success: true,
        message: "Вход выполнен".to_string(),
    })
}

pub async fn logout(session: Session) -> impl Responder {
    session.purge();
    HttpResponse::SeeOther()
        .insert_header((header::LOCATION, "/login"))
        .finish()
}
