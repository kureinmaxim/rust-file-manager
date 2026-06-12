use actix_session::{Session, SessionExt};
use actix_web::body::{BoxBody, MessageBody};
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::http::header;
use actix_web::middleware::Next;
use actix_web::{web, Error, HttpResponse, Responder};
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;
use crate::users::{validate_username, UserStore};

const SESSION_USER_KEY: &str = "user";
const SESSION_ADMIN_KEY: &str = "is_admin";

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

fn json_err(status: actix_web::http::StatusCode, message: impl Into<String>) -> HttpResponse {
    HttpResponse::build(status).json(LoginResponse { success: false, message: message.into() })
}

/// Logged-in user taken from the session cookie.
#[derive(Clone)]
pub struct CurrentUser {
    pub username: String,
    pub is_admin: bool,
}

pub fn current_user(session: &Session) -> Option<CurrentUser> {
    let username = session.get::<String>(SESSION_USER_KEY).ok().flatten()?;
    let is_admin = session.get::<bool>(SESSION_ADMIN_KEY).ok().flatten().unwrap_or(false);
    Some(CurrentUser { username, is_admin })
}

/// Middleware guard: every route behind it requires a logged-in session.
/// Browser navigation gets a redirect to /login, API calls get 401 JSON.
/// Sessions of deleted users are rejected even if the cookie is still valid.
pub async fn require_auth(
    req: ServiceRequest,
    next: Next<impl MessageBody + 'static>,
) -> Result<ServiceResponse<BoxBody>, Error> {
    let session = req.get_session();
    let config = req
        .app_data::<web::Data<AppConfig>>()
        .expect("AppConfig missing")
        .clone();
    let store = req
        .app_data::<web::Data<UserStore>>()
        .expect("UserStore missing")
        .clone();

    let logged_in = match current_user(&session) {
        Some(user) => user.username == config.admin_username || store.user_exists(&user.username),
        None => false,
    };

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
        json_err(actix_web::http::StatusCode::UNAUTHORIZED, "Требуется вход")
    };
    Ok(ServiceResponse::new(req, response))
}

/// Middleware guard for /admin routes: the session must belong to the admin.
pub async fn require_admin(
    req: ServiceRequest,
    next: Next<impl MessageBody + 'static>,
) -> Result<ServiceResponse<BoxBody>, Error> {
    let is_admin = current_user(&req.get_session()).is_some_and(|u| u.is_admin);
    if is_admin {
        return Ok(next.call(req).await?.map_into_boxed_body());
    }
    let (req, _) = req.into_parts();
    Ok(ServiceResponse::new(
        req,
        json_err(actix_web::http::StatusCode::FORBIDDEN, "Доступно только администратору"),
    ))
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
    store: web::Data<UserStore>,
) -> impl Responder {
    let username = form.username.trim().to_lowercase();
    let is_admin = username == config.admin_username
        && bcrypt::verify(&form.password, &config.admin_password_hash).unwrap_or(false);
    let is_user = !is_admin && store.verify_password(&username, &form.password);

    if !is_admin && !is_user {
        tracing::warn!(username = %username, "failed login attempt");
        return json_err(
            actix_web::http::StatusCode::UNAUTHORIZED,
            "Неверное имя пользователя или пароль",
        );
    }

    session.renew();
    if let Err(e) = session
        .insert(SESSION_USER_KEY, &username)
        .and_then(|_| session.insert(SESSION_ADMIN_KEY, is_admin))
    {
        return json_err(
            actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Ошибка сессии: {e}"),
        );
    }

    tracing::info!(username = %username, is_admin, "login successful");
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

#[derive(Deserialize)]
pub struct RegisterQuery {
    #[serde(default)]
    token: String,
}

#[derive(Deserialize)]
pub struct RegisterForm {
    token: String,
    username: String,
    password: String,
}

/// Registration page, reachable only with a valid invite token in the URL.
pub async fn register_page(
    query: web::Query<RegisterQuery>,
    store: web::Data<UserStore>,
) -> impl Responder {
    if !store.invite_valid(&query.token) {
        return HttpResponse::Gone()
            .content_type("text/html; charset=utf-8")
            .body(include_str!("../templates/invite_invalid.html"));
    }
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(include_str!("../templates/register.html"))
}

pub async fn register(
    form: web::Form<RegisterForm>,
    session: Session,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> impl Responder {
    let username = match validate_username(&form.username) {
        Ok(u) => u,
        Err(e) => return json_err(actix_web::http::StatusCode::BAD_REQUEST, e),
    };
    if username == config.admin_username || store.user_exists(&username) {
        return json_err(
            actix_web::http::StatusCode::BAD_REQUEST,
            "Пользователь с таким именем уже существует",
        );
    }
    if form.password.len() < 8 {
        return json_err(
            actix_web::http::StatusCode::BAD_REQUEST,
            "Пароль должен быть не короче 8 символов",
        );
    }

    // Consume the token first: each invite link registers exactly one account.
    if let Err(e) = store.take_invite(&form.token) {
        return json_err(actix_web::http::StatusCode::GONE, e);
    }
    if let Err(e) = store.add_user(&username, &form.password) {
        return json_err(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR, e);
    }

    tracing::info!(username = %username, "user registered via invite");

    // Log the new user in right away.
    session.renew();
    let _ = session.insert(SESSION_USER_KEY, &username);
    let _ = session.insert(SESSION_ADMIN_KEY, false);

    HttpResponse::Ok().json(LoginResponse {
        success: true,
        message: "Аккаунт создан".to_string(),
    })
}
