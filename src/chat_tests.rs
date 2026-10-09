//! Files through the chat: saving attachments the bot forwards and the
//! «Отправить в чат» queue, on a real actix service with synthetic data.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{header, StatusCode};
use actix_web::{test, web, App};
use serde_json::{json, Value};

use crate::chat::ChatJobs;
use crate::config::AppConfig;
use crate::paths::RelPath;
use crate::storage::{encode_id, Zone, STAGING_DIR};
use crate::tg_auth::test_support::{init_data, user_json};
use crate::tokens::TokenKeys;
use crate::users::UserStore;

const ANNA: i64 = 101;
const BOB: i64 = 202;
const STRANGER: i64 = 909;
const SERVICE: &str = "internal-test-token-0123456789abcdef";

struct Env {
    dir: PathBuf,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    keys: web::Data<TokenKeys>,
    jobs: web::Data<ChatJobs>,
}

impl Env {
    fn new(with_bot: bool) -> Self {
        let dir = crate::storage::test_directory("chat");
        let mut config = AppConfig::for_tests(dir.join("uploads"));
        if with_bot {
            config.internal_api_token = Some(SERVICE.into());
        }
        let store = UserStore::load(dir.join("users.json")).unwrap();
        store.add_user("anna", "anna-password").unwrap();
        store.add_user("bob", "bob-password").unwrap();
        store.link_telegram("anna", "admin", ANNA).unwrap();
        store.link_telegram("bob", "admin", BOB).unwrap();
        let keys = TokenKeys::derive(&config.secret);
        Self {
            dir,
            config: web::Data::new(config),
            store: web::Data::new(store),
            keys: web::Data::new(keys),
            jobs: web::Data::new(ChatJobs::default()),
        }
    }

    async fn app(
        &self,
    ) -> impl Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>
    {
        test::init_service(
            App::new()
                .app_data(self.config.clone())
                .app_data(self.store.clone())
                .app_data(self.keys.clone())
                .app_data(self.jobs.clone())
                .app_data(web::Data::new(crate::api::link_limiter()))
                .app_data(web::Data::new(crate::uploads::UploadLocks::default()))
                .app_data(web::Data::new(crate::exchange::Events::default()))
                .app_data(web::Data::new(crate::internal::ServerInfo {
                    started: Instant::now(),
                }))
                .configure(crate::api::configure)
                .configure(crate::internal::configure),
        )
        .await
    }

    fn uploads(&self) -> PathBuf {
        self.dir.join("uploads")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

async fn send<S>(app: &S, req: test::TestRequest) -> (StatusCode, Value, Vec<u8>)
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let resp = test::call_service(app, req.to_request()).await;
    let status = resp.status();
    let bytes = test::read_body(resp).await.to_vec();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json, bytes)
}

fn service(req: test::TestRequest) -> test::TestRequest {
    req.insert_header((header::AUTHORIZATION, format!("Bearer {SERVICE}")))
}

fn save(tg: i64, query: &str, body: &'static [u8]) -> test::TestRequest {
    service(
        test::TestRequest::post()
            .uri(&format!("/internal/v1/files?telegram_id={tg}&{query}"))
            .insert_header((header::CONTENT_TYPE, "application/octet-stream"))
            .set_payload(body),
    )
}

async fn login<S>(app: &S, tg_id: i64) -> String
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let user = user_json(tg_id, "u");
    let now = crate::reply::now_secs().to_string();
    let data = init_data(&[("auth_date", now.as_str()), ("user", user.as_str())]);
    let (status, body, _) = send(
        app,
        test::TestRequest::post()
            .uri("/api/v1/tg/session")
            .set_json(json!({ "init_data": data })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["access_token"].as_str().unwrap().to_string()
}

fn bearer(req: test::TestRequest, token: &str) -> test::TestRequest {
    req.insert_header((header::AUTHORIZATION, format!("Bearer {token}")))
}

#[actix_web::test]
async fn saves_attachment_into_the_linked_account_without_overwriting() {
    let env = Env::new(true);
    let app = env.app().await;

    let (status, body, _) = send(
        &app,
        save(
            ANNA,
            "name=%D0%BE%D1%82%D1%87%D1%91%D1%82.pdf",
            b"synthetic-one",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["item"]["scope"], "my");
    assert_eq!(body["item"]["category"], "Документы");
    assert_eq!(body["item"]["name"], "отчёт.pdf");
    assert_eq!(body["message"], "Сохранено в «Мои файлы › Документы»");
    let docs = env.uploads().join("home/anna/Документы");
    assert_eq!(fs::read(docs.join("отчёт.pdf")).unwrap(), b"synthetic-one");

    // The same name again: a second file, the first one untouched.
    let (_, body, _) = send(
        &app,
        save(
            ANNA,
            "name=%D0%BE%D1%82%D1%87%D1%91%D1%82.pdf",
            b"synthetic-two",
        ),
    )
    .await;
    assert_eq!(body["item"]["name"], "отчёт(1).pdf");
    assert_eq!(fs::read(docs.join("отчёт.pdf")).unwrap(), b"synthetic-one");

    // Shared zone, and a name with a path keeps only its last component.
    let (status, body, _) = send(
        &app,
        save(
            BOB,
            "name=..%2F..%2Fphoto.jpg&scope=shared",
            b"synthetic-photo",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["item"]["name"], "photo.jpg");
    assert!(env.uploads().join("shared/Фото/photo.jpg").is_file());
    assert!(!env.dir.join("photo.jpg").exists());

    // Nothing is left in staging after successful saves.
    let staged: Vec<_> = fs::read_dir(env.uploads().join(STAGING_DIR).join("anna"))
        .unwrap()
        .collect();
    assert!(staged.is_empty());
}

#[actix_web::test]
async fn rejects_unlinked_oversized_and_unauthenticated_saves() {
    let env = Env::new(true);
    let app = env.app().await;

    let (status, body, _) = send(&app, save(STRANGER, "name=a.txt", b"x")).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("not_linked"))
    );

    let (status, body, _) = send(&app, save(ANNA, "name=a.txt&scope=other", b"x")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    static BIG: [u8; 11 * 1024 * 1024] = [0u8; 11 * 1024 * 1024];
    let (status, body, _) = send(&app, save(ANNA, "name=big.bin", &BIG)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, Some("too_large"))
    );
    assert!(!env.uploads().join("home/anna/Другие/big.bin").exists());

    let (status, _, _) = send(
        &app,
        test::TestRequest::post()
            .uri(&format!("/internal/v1/files?telegram_id={ANNA}&name=a.txt"))
            .set_payload(&b"x"[..]),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[actix_web::test]
async fn send_to_chat_queues_one_job_for_the_requesting_account() {
    let env = Env::new(true);
    let app = env.app().await;
    let docs = env.uploads().join("home/anna/Документы");
    fs::create_dir_all(&docs).unwrap();
    fs::write(docs.join("план.txt"), b"synthetic-plan").unwrap();
    let id = encode_id(Zone::My, "Документы", &RelPath::root(), Some("план.txt"));

    let anna = login(&app, ANNA).await;
    let (_, me, _) = send(
        &app,
        bearer(test::TestRequest::get().uri("/api/v1/me"), &anna),
    )
    .await;
    assert_eq!(me["features"]["send_to_chat"], true);

    // Bob's token resolves the same id inside his own zone: nothing there.
    let bob = login(&app, BOB).await;
    let (status, _, _) = send(
        &app,
        bearer(
            test::TestRequest::post().uri(&format!("/api/v1/files/{id}/send-to-chat")),
            &bob,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body, _) = send(
        &app,
        bearer(
            test::TestRequest::post().uri(&format!("/api/v1/files/{id}/send-to-chat")),
            &anna,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["message"], "Файл придёт в чат с ботом");

    let (status, body, _) = send(
        &app,
        service(test::TestRequest::get().uri("/internal/v1/jobs?wait=1")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let jobs = body["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 1, "{body}");
    assert_eq!(jobs[0]["telegram_id"], ANNA);
    assert_eq!(jobs[0]["name"], "план.txt");
    assert_eq!(jobs[0]["size"], 14);
    let job = jobs[0]["id"].as_u64().unwrap();

    let (status, _, bytes) = send(
        &app,
        service(test::TestRequest::get().uri(&format!("/internal/v1/jobs/{job}/content"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"synthetic-plan");

    let (status, _, _) = send(
        &app,
        service(test::TestRequest::post().uri(&format!("/internal/v1/jobs/{job}/done")))
            .set_json(json!({ "ok": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body, _) = send(
        &app,
        service(test::TestRequest::get().uri(&format!("/internal/v1/jobs/{job}/content"))),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("no_job"))
    );

    // The queue is empty again: a short long-poll returns nothing.
    let started = Instant::now();
    let (_, body, _) = send(
        &app,
        service(test::TestRequest::get().uri("/internal/v1/jobs?wait=1")),
    )
    .await;
    assert_eq!(body["jobs"], json!([]));
    assert!(started.elapsed().as_millis() >= 900);
}

#[actix_web::test]
async fn deleted_file_is_not_sent() {
    let env = Env::new(true);
    let app = env.app().await;
    let docs = env.uploads().join("home/anna/Документы");
    fs::create_dir_all(&docs).unwrap();
    fs::write(docs.join("a.txt"), b"synthetic").unwrap();
    let id = encode_id(Zone::My, "Документы", &RelPath::root(), Some("a.txt"));
    let anna = login(&app, ANNA).await;
    send(
        &app,
        bearer(
            test::TestRequest::post().uri(&format!("/api/v1/files/{id}/send-to-chat")),
            &anna,
        ),
    )
    .await;
    let (_, body, _) = send(
        &app,
        service(test::TestRequest::get().uri("/internal/v1/jobs")),
    )
    .await;
    let job = body["jobs"][0]["id"].as_u64().unwrap();
    fs::remove_file(docs.join("a.txt")).unwrap();
    let (status, _, _) = send(
        &app,
        service(test::TestRequest::get().uri(&format!("/internal/v1/jobs/{job}/content"))),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[actix_web::test]
async fn without_the_bot_sending_to_chat_is_off() {
    let env = Env::new(false);
    let app = env.app().await;
    let docs = env.uploads().join("home/anna/Документы");
    fs::create_dir_all(&docs).unwrap();
    fs::write(docs.join("a.txt"), b"synthetic").unwrap();
    let id = encode_id(Zone::My, "Документы", &RelPath::root(), Some("a.txt"));
    let anna = login(&app, ANNA).await;
    let (_, me, _) = send(
        &app,
        bearer(test::TestRequest::get().uri("/api/v1/me"), &anna),
    )
    .await;
    assert_eq!(me["features"]["send_to_chat"], false);
    let (status, body, _) = send(
        &app,
        bearer(
            test::TestRequest::post().uri(&format!("/api/v1/files/{id}/send-to-chat")),
            &anna,
        ),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("disabled"))
    );
    assert!(env.jobs.take(5).is_empty());
}
