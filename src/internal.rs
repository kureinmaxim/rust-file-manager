//! Internal API for the companion Telegram bot (TelegramOnly).
//!
//! Served on a separate listener (`INTERNAL_BIND_ADDR`, normally
//! 127.0.0.1:8091) that nginx never proxies; the bot reaches it through a
//! socat bridge on the docker gateway. Every request must carry
//! `Authorization: Bearer <INTERNAL_API_TOKEN>`. The bot is trusted to pass
//! Telegram ids taken from Telegram updates; admin-only operations still
//! require that id to be linked to the file manager's admin.

use std::time::Instant;

use actix_web::body::{BoxBody, MessageBody};
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::http::{header, StatusCode};
use actix_web::middleware::{from_fn, Next};
use actix_web::{get, post, web, Error, HttpResponse};
use serde::Deserialize;
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::config::AppConfig;
use crate::exchange::{Events, Inbox};
use crate::reply;
use crate::storage::{disk_usage, folder_size, STAGING_DIR};
use crate::users::{LinkError, UserStore};

/// Process start, for `uptime_secs`.
pub struct ServerInfo {
    pub started: Instant,
}

/// Register the internal routes (used by main and by tests).
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/internal/v1")
            .wrap(from_fn(require_service_token))
            .service(get_health)
            .service(get_status)
            .service(get_tg_links)
            .service(get_resolve)
            .service(post_invite)
            .service(post_bind)
            .service(post_unbind)
            .service(get_events)
            .configure(crate::chat::configure_internal),
    );
}

async fn require_service_token(
    req: ServiceRequest,
    next: Next<impl MessageBody + 'static>,
) -> Result<ServiceResponse<BoxBody>, Error> {
    let expected = req
        .app_data::<web::Data<AppConfig>>()
        .and_then(|c| c.internal_api_token.clone());
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);
    let authorized = match (expected, presented) {
        (Some(expected), Some(presented)) => {
            bool::from(expected.as_bytes().ct_eq(presented.as_bytes()))
        }
        _ => false,
    };
    if authorized {
        return Ok(next.call(req).await?.map_into_boxed_body());
    }
    tracing::warn!(path = %req.path(), "internal API: rejected service token");
    let (req, _) = req.into_parts();
    Ok(ServiceResponse::new(
        req,
        reply::error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Неверный служебный токен",
        ),
    ))
}

#[get("/health")]
async fn get_health() -> HttpResponse {
    reply::ok(json!({ "version": env!("CARGO_PKG_VERSION") }))
}

#[get("/status")]
async fn get_status(
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    info: web::Data<ServerInfo>,
) -> HttpResponse {
    let disk = disk_usage(&config.upload_dir)
        .map(|(total, free)| json!({ "total_bytes": total, "free_bytes": free }));
    reply::ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "commit": env!("BUILD_GIT_COMMIT"),
        "uptime_secs": info.started.elapsed().as_secs(),
        "disk": disk,
        // Participants: registered users plus the admin.
        "users": store.user_count() + 1,
        "telegram_linked": store.telegram_links(&config.admin_username).len(),
        "staging_bytes": folder_size(&config.upload_dir.join(STAGING_DIR)),
        "trash_bytes": 0,
        "active_shares": 0,
    }))
}

#[get("/tg-links")]
async fn get_tg_links(config: web::Data<AppConfig>, store: web::Data<UserStore>) -> HttpResponse {
    let links: Vec<_> = store
        .telegram_links(&config.admin_username)
        .into_iter()
        .filter_map(|a| {
            a.telegram_id.map(
                |id| json!({ "telegram_id": id, "username": a.username, "is_admin": a.is_admin }),
            )
        })
        .collect();
    reply::ok(json!({ "links": links }))
}

#[derive(Deserialize)]
struct ResolveQuery {
    telegram_id: i64,
}

#[get("/resolve")]
async fn get_resolve(
    query: web::Query<ResolveQuery>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    match store.account_by_telegram(query.telegram_id, &config.admin_username) {
        Some(a) => {
            reply::ok(json!({ "linked": true, "username": a.username, "is_admin": a.is_admin }))
        }
        None => reply::ok(json!({ "linked": false })),
    }
}

fn not_admin() -> HttpResponse {
    reply::error(
        StatusCode::FORBIDDEN,
        "not_admin",
        "Telegram не привязан к администратору файлового менеджера",
    )
}

/// Username of the admin account linked to `telegram_id`, if any.
fn admin_by_telegram(store: &UserStore, config: &AppConfig, telegram_id: i64) -> Option<String> {
    store
        .account_by_telegram(telegram_id, &config.admin_username)
        .filter(|a| a.is_admin)
        .map(|a| a.username)
}

#[derive(Deserialize)]
struct InviteBody {
    telegram_id: i64,
}

#[post("/invites")]
async fn post_invite(
    body: web::Json<InviteBody>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    let Some(admin) = admin_by_telegram(&store, &config, body.telegram_id) else {
        return not_admin();
    };
    let invite = match store.create_invite(&admin) {
        Ok(i) => i,
        Err(e) => return reply::error(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e),
    };
    let tg_url = config.telegram.as_ref().map(|t| {
        format!(
            "https://t.me/{}?startapp=inv_{}",
            t.bot_username, invite.token
        )
    });
    let web_url = config
        .public_base_url
        .as_ref()
        .map(|base| format!("{base}/register?token={}", invite.token));
    tracing::info!(created_by = %admin, "invite link created via bot");
    reply::ok(json!({
        "token": invite.token,
        "expires_at": invite.expires_at,
        "tg_url": tg_url,
        "web_url": web_url,
    }))
}

fn link_error(e: LinkError) -> HttpResponse {
    match e {
        LinkError::NoUser => {
            reply::error(StatusCode::NOT_FOUND, "no_user", "Пользователь не найден")
        }
        LinkError::AlreadyLinked => reply::error(
            StatusCode::CONFLICT,
            "already_linked",
            "Telegram или аккаунт уже привязаны",
        ),
        LinkError::NotLinked => reply::error(
            StatusCode::NOT_FOUND,
            "not_linked",
            "Этот Telegram не привязан",
        ),
        LinkError::Save(e) => reply::error(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e),
    }
}

#[derive(Deserialize)]
struct BindBody {
    admin_telegram_id: i64,
    telegram_id: i64,
    username: String,
}

#[post("/bind")]
async fn post_bind(
    body: web::Json<BindBody>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    let Some(admin) = admin_by_telegram(&store, &config, body.admin_telegram_id) else {
        return not_admin();
    };
    let username = body.username.trim().to_lowercase();
    if let Err(e) = store.link_telegram(&username, &config.admin_username, body.telegram_id) {
        return link_error(e);
    }
    tracing::info!(by = %admin, user = %username, "telegram linked by admin");
    reply::ok(json!({ "message": "Telegram привязан", "username": username }))
}

#[derive(Deserialize)]
struct UnbindBody {
    admin_telegram_id: i64,
    telegram_id: i64,
}

#[post("/unbind")]
async fn post_unbind(
    body: web::Json<UnbindBody>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
) -> HttpResponse {
    let Some(admin) = admin_by_telegram(&store, &config, body.admin_telegram_id) else {
        return not_admin();
    };
    match store.unlink_telegram(body.telegram_id, &config.admin_username) {
        Ok(username) => {
            tracing::info!(by = %admin, user = %username, "telegram unlinked by admin");
            reply::ok(json!({ "message": "Telegram отвязан", "username": username }))
        }
        Err(e) => link_error(e),
    }
}

#[derive(Deserialize)]
struct EventsQuery {
    after: Option<u64>,
}

/// Files that arrived in exchanges since `after`, with the Telegram accounts
/// to notify (the bot polls this and writes to them). Without `after` only the
/// newest id is returned, so a freshly started bot does not replay old events.
#[get("/events")]
async fn get_events(
    query: web::Query<EventsQuery>,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    events: web::Data<Events>,
) -> HttpResponse {
    let Some(after) = query.after else {
        return reply::ok(json!({ "events": [], "last_id": events.last_id() }));
    };
    let batch = events.after(after, 100);
    let last_id = batch.last().map_or(after, |e| e.id);
    let list: Vec<_> = batch
        .iter()
        .filter_map(|e| {
            // The recipient is the other side; a deleted user has no exchange left.
            let owner = store.account(&e.owner, &config.admin_username)?;
            let (sender, recipient) = match e.inbox {
                Inbox::FromAdmin => (config.admin_username.clone(), owner),
                Inbox::FromUser => (
                    e.owner.clone(),
                    store.account(&config.admin_username, &config.admin_username)?,
                ),
            };
            let recipients: Vec<_> = recipient
                .telegram_id
                .map(|id| json!({ "telegram_id": id, "username": recipient.username, "is_admin": recipient.is_admin }))
                .into_iter()
                .collect();
            Some(json!({
                "id": e.id,
                "type": "exchange.file",
                "ts": e.ts,
                "owner": e.owner,
                "direction": e.inbox.as_str(),
                "sender": sender,
                "name": e.name,
                "size": e.size,
                "recipients": recipients,
            }))
        })
        .collect();
    reply::ok(json!({ "events": list, "last_id": last_id }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, App};
    use serde_json::Value;

    const TOKEN: &str = "internal-test-token-0123456789abcdef";

    struct Env {
        config: web::Data<AppConfig>,
        store: web::Data<UserStore>,
        events: web::Data<Events>,
    }

    fn env() -> Env {
        let dir = crate::storage::test_directory("internal");
        let mut config = AppConfig::for_tests(dir.clone());
        config.internal_api_token = Some(TOKEN.into());
        let store = UserStore::load(dir.join("users.json")).unwrap();
        store.add_user("bob", "secret-pass").unwrap();
        store.link_telegram("admin", "admin", 111).unwrap();
        store.link_telegram("bob", "admin", 222).unwrap();
        Env {
            config: web::Data::new(config),
            store: web::Data::new(store),
            events: web::Data::new(Events::default()),
        }
    }

    async fn call(env: &Env, req: test::TestRequest, token: Option<&str>) -> (StatusCode, Value) {
        let app = test::init_service(
            App::new()
                .app_data(env.config.clone())
                .app_data(env.store.clone())
                .app_data(env.events.clone())
                .app_data(web::Data::new(ServerInfo {
                    started: Instant::now(),
                }))
                .configure(configure),
        )
        .await;
        let req = match token {
            Some(t) => req.insert_header((header::AUTHORIZATION, format!("Bearer {t}"))),
            None => req,
        };
        let resp = test::call_service(&app, req.to_request()).await;
        let status = resp.status();
        let body: Value = test::read_body_json(resp).await;
        (status, body)
    }

    #[actix_web::test]
    async fn rejects_missing_and_wrong_token() {
        let env = env();
        for token in [
            None,
            Some("wrong"),
            Some("internal-test-token-0123456789abcdeX"),
        ] {
            let (status, body) = call(
                &env,
                test::TestRequest::get().uri("/internal/v1/health"),
                token,
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body["code"], "unauthorized");
        }
    }

    #[actix_web::test]
    async fn health_status_links_resolve() {
        let env = env();
        let (s, b) = call(
            &env,
            test::TestRequest::get().uri("/internal/v1/health"),
            Some(TOKEN),
        )
        .await;
        assert_eq!(
            (s, b["success"].clone()),
            (StatusCode::OK, Value::Bool(true))
        );
        assert_eq!(b["version"], env!("CARGO_PKG_VERSION"));

        let (_, b) = call(
            &env,
            test::TestRequest::get().uri("/internal/v1/status"),
            Some(TOKEN),
        )
        .await;
        assert_eq!(b["users"], 2);
        assert_eq!(b["telegram_linked"], 2);
        assert!(b["uptime_secs"].is_u64());

        let (_, b) = call(
            &env,
            test::TestRequest::get().uri("/internal/v1/tg-links"),
            Some(TOKEN),
        )
        .await;
        let links = b["links"].as_array().unwrap();
        assert!(links.contains(&json!({"telegram_id": 111, "username": "admin", "is_admin": true})));
        assert!(links.contains(&json!({"telegram_id": 222, "username": "bob", "is_admin": false})));

        let (_, b) = call(
            &env,
            test::TestRequest::get().uri("/internal/v1/resolve?telegram_id=222"),
            Some(TOKEN),
        )
        .await;
        assert_eq!(
            b,
            json!({"success": true, "linked": true, "username": "bob", "is_admin": false})
        );
        let (_, b) = call(
            &env,
            test::TestRequest::get().uri("/internal/v1/resolve?telegram_id=5"),
            Some(TOKEN),
        )
        .await;
        assert_eq!(b, json!({"success": true, "linked": false}));
    }

    #[actix_web::test]
    async fn invites_require_fm_admin() {
        let env = env();
        let req = |id: i64| {
            test::TestRequest::post()
                .uri("/internal/v1/invites")
                .set_json(json!({"telegram_id": id}))
        };
        let (s, b) = call(&env, req(222), Some(TOKEN)).await;
        assert_eq!(
            (s, b["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("not_admin"))
        );
        let (s, b) = call(&env, req(111), Some(TOKEN)).await;
        assert_eq!(s, StatusCode::OK);
        let token = b["token"].as_str().unwrap();
        assert_eq!(
            b["tg_url"],
            format!("https://t.me/files_test_bot?startapp=inv_{token}")
        );
        assert_eq!(
            b["web_url"],
            format!("https://files.example.com/register?token={token}")
        );
        assert!(env.store.invite_valid(token));
    }

    #[actix_web::test]
    async fn bind_and_unbind() {
        let env = env();
        env.store.add_user("sergey", "secret-pass").unwrap();
        let bind_req = |admin: i64, tg: i64, user: &str| {
            test::TestRequest::post()
                .uri("/internal/v1/bind")
                .set_json(json!({"admin_telegram_id": admin, "telegram_id": tg, "username": user}))
        };
        let (s, b) = call(&env, bind_req(222, 444, "sergey"), Some(TOKEN)).await;
        assert_eq!(
            (s, b["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("not_admin"))
        );
        let (s, b) = call(&env, bind_req(111, 444, "Sergey"), Some(TOKEN)).await;
        assert_eq!(
            (s, b["username"].as_str()),
            (StatusCode::OK, Some("sergey"))
        );
        let (s, b) = call(&env, bind_req(111, 444, "bob"), Some(TOKEN)).await;
        assert_eq!(
            (s, b["code"].as_str()),
            (StatusCode::CONFLICT, Some("already_linked"))
        );
        let (s, b) = call(&env, bind_req(111, 555, "nobody"), Some(TOKEN)).await;
        assert_eq!(
            (s, b["code"].as_str()),
            (StatusCode::NOT_FOUND, Some("no_user"))
        );

        let unbind_req = |tg: i64| {
            test::TestRequest::post()
                .uri("/internal/v1/unbind")
                .set_json(json!({"admin_telegram_id": 111, "telegram_id": tg}))
        };
        let (s, b) = call(&env, unbind_req(444), Some(TOKEN)).await;
        assert_eq!(
            (s, b["username"].as_str()),
            (StatusCode::OK, Some("sergey"))
        );
        assert_eq!(
            env.store.account("sergey", "admin").unwrap().token_version,
            1
        );
        let (s, b) = call(&env, unbind_req(444), Some(TOKEN)).await;
        assert_eq!(
            (s, b["code"].as_str()),
            (StatusCode::NOT_FOUND, Some("not_linked"))
        );
    }

    #[actix_web::test]
    async fn events_name_the_other_side_of_the_exchange() {
        let env = env();
        let get = |uri: &str| test::TestRequest::get().uri(uri);

        // A fresh reader starts at the newest id and gets nothing old.
        let (status, start) = call(&env, get("/internal/v1/events"), Some(TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(start["events"], json!([]));
        let start_id = start["last_id"].as_u64().unwrap();

        env.events.push("bob", Inbox::FromAdmin, "план.pdf", 10);
        env.events.push("ghost", Inbox::FromAdmin, "x.pdf", 1); // no such user: skipped
        env.events.push("bob", Inbox::FromUser, "ответ.txt", 2);

        let (_, body) = call(
            &env,
            get(&format!("/internal/v1/events?after={start_id}")),
            Some(TOKEN),
        )
        .await;
        let events = body["events"].as_array().unwrap();
        assert_eq!(events.len(), 2, "{body}");
        assert_eq!(events[0]["direction"], "from-admin");
        assert_eq!(events[0]["sender"], "admin");
        assert_eq!(events[0]["name"], "план.pdf");
        assert_eq!(
            events[0]["recipients"],
            json!([{ "telegram_id": 222, "username": "bob", "is_admin": false }])
        );
        assert_eq!(events[1]["direction"], "from-user");
        assert_eq!(events[1]["sender"], "bob");
        assert_eq!(events[1]["recipients"][0]["telegram_id"], 111);
        assert_eq!(events[1]["recipients"][0]["is_admin"], true);
        let last = body["last_id"].as_u64().unwrap();
        assert!(last > start_id);

        let (_, again) = call(
            &env,
            get(&format!("/internal/v1/events?after={last}")),
            Some(TOKEN),
        )
        .await;
        assert_eq!(again["events"], json!([]));
        assert_eq!(again["last_id"], last);
        let (status, _) = call(&env, get("/internal/v1/events?after=0"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
