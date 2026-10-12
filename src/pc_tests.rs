//! «Загрузить с ПК» on a real actix service: pairing, approval from the Mini
//! App, what a computer token may and may not do, disconnecting, and the
//! free-space check of new uploads.

use std::fs;
use std::path::PathBuf;

use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{header, StatusCode};
use actix_web::{test, web, App};
use serde_json::{json, Value};

use crate::api::{self, link_limiter};
use crate::config::AppConfig;
use crate::pc::Computers;
use crate::tg_auth::test_support::{init_data, user_json};
use crate::tokens::TokenKeys;
use crate::uploads::UploadLocks;
use crate::users::UserStore;

const ANNA: i64 = 101;
const BOB: i64 = 202;
const CHROME_WINDOWS: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

struct Env {
    dir: PathBuf,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    keys: web::Data<TokenKeys>,
    computers: web::Data<Computers>,
}

impl Env {
    fn new() -> Self {
        Self::with(|_| {})
    }

    fn with(adjust: impl FnOnce(&mut AppConfig)) -> Self {
        let dir = crate::storage::test_directory("pc");
        let mut config = AppConfig::for_tests(dir.join("uploads"));
        adjust(&mut config);
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
            computers: web::Data::new(Computers::default()),
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
                .app_data(web::Data::new(link_limiter()))
                .app_data(web::Data::new(UploadLocks::default()))
                .app_data(web::Data::new(crate::exchange::Events::default()))
                .app_data(self.computers.clone())
                .configure(api::configure),
        )
        .await
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

async fn send<S>(app: &S, req: test::TestRequest) -> (StatusCode, Value)
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let resp = test::call_service(app, req.to_request()).await;
    let status = resp.status();
    let bytes = test::read_body(resp).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn authed(req: test::TestRequest, token: &str) -> test::TestRequest {
    req.insert_header((header::AUTHORIZATION, format!("Bearer {token}")))
}

async fn login<S>(app: &S, tg_id: i64) -> String
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let now = crate::reply::now_secs().to_string();
    let user = user_json(tg_id, "u");
    let init = init_data(&[("auth_date", now.as_str()), ("user", user.as_str())]);
    let (status, body) = send(
        app,
        test::TestRequest::post()
            .uri("/api/v1/tg/session")
            .set_json(json!({ "init_data": init })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["access_token"].as_str().unwrap().to_string()
}

/// A computer asks for a code: `(secret, code as shown)`.
async fn pair<S>(app: &S) -> (String, String)
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    pair_as(app, "").await
}

/// The same from a browser with this device id.
async fn pair_as<S>(app: &S, device: &str) -> (String, String)
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let (status, body) = send(
        app,
        test::TestRequest::post()
            .uri("/api/v1/pc/pair")
            .insert_header((header::USER_AGENT, CHROME_WINDOWS))
            .set_json(json!({ "device": device })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (
        body["secret"].as_str().unwrap().to_string(),
        body["code"].as_str().unwrap().to_string(),
    )
}

async fn poll<S>(app: &S, secret: &str) -> (StatusCode, Value)
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    send(
        app,
        test::TestRequest::post()
            .uri("/api/v1/pc/poll")
            .set_json(json!({ "secret": secret })),
    )
    .await
}

/// Pair a computer and approve it as the owner of `app_token`: the computer
/// token and the session id.
async fn connect<S>(app: &S, app_token: &str) -> (String, String)
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    connect_as(app, app_token, "").await
}

async fn connect_as<S>(app: &S, app_token: &str, device: &str) -> (String, String)
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let (secret, code) = pair_as(app, device).await;
    let (status, body) = send(
        app,
        authed(
            test::TestRequest::post().uri("/api/v1/computers/approve"),
            app_token,
        )
        .set_json(json!({ "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = poll(app, &secret).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "approved");
    (
        body["access_token"].as_str().unwrap().to_string(),
        body["computer"]["id"].as_str().unwrap().to_string(),
    )
}

async fn create<S>(app: &S, token: &str, body: Value) -> (StatusCode, Value)
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    send(
        app,
        authed(test::TestRequest::post().uri("/api/v1/uploads"), token).set_json(body),
    )
    .await
}

/// Upload `data` as `name` through the chunked protocol; the finished item.
async fn upload<S>(app: &S, token: &str, name: &str, data: &[u8]) -> Value
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let (status, created) = create(
        app,
        token,
        json!({ "scope": "my", "name": name, "size": data.len() }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap();
    let (status, reply) = send(
        app,
        authed(
            test::TestRequest::put().uri(&format!("/api/v1/uploads/{id}")),
            token,
        )
        .insert_header(("Upload-Offset", "0"))
        .set_payload(data.to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let (status, done) = send(
        app,
        authed(
            test::TestRequest::post().uri(&format!("/api/v1/uploads/{id}/complete")),
            token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done["item"].clone()
}

#[actix_web::test]
async fn computer_pairs_with_a_code_and_uploads_into_my_files() {
    let env = Env::new();
    let app = env.app().await;

    let (status, body) = send(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/pc/pair")
            .insert_header((header::USER_AGENT, CHROME_WINDOWS))
            .set_json(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let secret = body["secret"].as_str().unwrap().to_string();
    let code = body["code"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 9, "ABCD-EFGH: {code}");
    assert_eq!(&code[4..5], "-");
    let link = body["link"].as_str().unwrap();
    assert_eq!(
        link,
        format!(
            "https://t.me/files_test_bot?startapp=pc_{}",
            code.replace('-', "")
        )
    );
    assert!(body["qr"]["size"].as_u64().unwrap() >= 21);
    assert!(body["qr"]["path"].as_str().unwrap().starts_with("M0 0h7"));
    assert_eq!(body["expires_in"], 300);

    let (status, body) = poll(&app, &secret).await;
    assert_eq!(
        (status, body["status"].as_str()),
        (StatusCode::OK, Some("pending"))
    );
    assert!(body.get("access_token").is_none());

    // The phone looks the code up (typed in lowercase, without the dash).
    let anna = login(&app, ANNA).await;
    let (status, body) = send(
        &app,
        authed(
            test::TestRequest::post().uri("/api/v1/computers/lookup"),
            &anna,
        )
        .set_json(json!({ "code": code.replace('-', "").to_lowercase() })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], code.as_str());
    assert_eq!(body["agent"], "Chrome · Windows");

    // …or scans the QR code: the whole link works as the code.
    let (status, body) = send(
        &app,
        authed(
            test::TestRequest::post().uri("/api/v1/computers/approve"),
            &anna,
        )
        .set_json(json!({ "code": link })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let session_id = body["computer"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["computer"]["agent"], "Chrome · Windows");

    let (status, body) = poll(&app, &secret).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "approved");
    assert_eq!(body["user"]["username"], "anna");
    assert_eq!(body["computer"]["id"], session_id.as_str());
    assert!(body["capacity"]["max_file_size"].as_u64().unwrap() > 0);
    let pc = body["access_token"].as_str().unwrap().to_string();
    assert!(pc.starts_with("pc1."));
    // The token is handed out once.
    let (status, body) = poll(&app, &secret).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::GONE, Some("pair_expired"))
    );

    let (status, capacity) = send(
        &app,
        authed(
            test::TestRequest::get().uri("/api/v1/uploads/capacity"),
            &pc,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{capacity}");
    assert_eq!(capacity["max_file_size"], 64 * 1024 * 1024);
    assert_eq!(capacity["chunk_size"], 1024 * 1024);

    // Files go to «Мои файлы», category by type.
    let photo = upload(&app, &pc, "дача.jpg", b"jpeg bytes").await;
    assert_eq!(photo["scope"], "my");
    assert_eq!(photo["category"], "Фото");
    let doc = upload(&app, &pc, "отчёт.pdf", b"%PDF").await;
    assert_eq!(doc["category"], "Документы");
    let on_disk = env.config.upload_dir.join("home/anna/Фото/дача.jpg");
    assert_eq!(fs::read(on_disk).unwrap(), b"jpeg bytes");

    let (status, own) = send(
        &app,
        authed(test::TestRequest::get().uri("/api/v1/pc/session"), &pc),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{own}");
    assert_eq!(own["user"]["username"], "anna");
    assert_eq!(own["computer"]["files"], 2);
    assert_eq!(own["computer"]["bytes"], 14);

    let (status, list) = send(
        &app,
        authed(test::TestRequest::get().uri("/api/v1/computers"), &anna),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["computers"].as_array().unwrap().len(), 1);
    assert_eq!(list["computers"][0]["files"], 2);
    assert_eq!(list["pc_url"], "https://files.example.com/pc");
}

#[actix_web::test]
async fn computer_token_only_uploads() {
    let env = Env::new();
    let app = env.app().await;
    let anna = login(&app, ANNA).await;
    let (pc, _) = connect(&app, &anna).await;

    // Only «Мои файлы» by file type.
    for body in [
        json!({ "scope": "shared", "name": "a.jpg", "size": 1 }),
        json!({ "scope": "my", "category": "Видео", "name": "a.jpg", "size": 1 }),
        json!({ "scope": "my", "path": "папка", "name": "a.jpg", "size": 1 }),
        json!({ "scope": "exchange", "name": "a.jpg", "size": 1 }),
    ] {
        let (status, reply) = create(&app, &pc, body.clone()).await;
        assert_eq!(
            (status, reply["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("pc_scope")),
            "{body}"
        );
    }

    // Nothing to read or change: listing, links, renames, exchange, sessions.
    for req in [
        test::TestRequest::get().uri("/api/v1/me"),
        test::TestRequest::get().uri("/api/v1/overview"),
        test::TestRequest::get().uri("/api/v1/files?scope=my"),
        test::TestRequest::post()
            .uri("/api/v1/files/abc/link")
            .set_json(json!({})),
        test::TestRequest::delete().uri("/api/v1/files/abc"),
        test::TestRequest::get().uri("/api/v1/exchange"),
        test::TestRequest::get().uri("/api/v1/computers"),
        test::TestRequest::post()
            .uri("/api/v1/computers/approve")
            .set_json(json!({ "code": "ABCD2345" })),
        test::TestRequest::post().uri("/api/v1/sessions/revoke-all"),
        test::TestRequest::post().uri("/api/v1/tg/unlink"),
    ] {
        let (status, reply) = send(&app, authed(req, &pc)).await;
        assert_eq!(
            (status, reply["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("pc_forbidden"))
        );
    }
    // Paths that only look like the upload API do not reach other handlers.
    for uri in [
        "/api/v1/uploads/../files?scope=my",
        "/api/v1/uploads/..%2Ffiles?scope=my",
        "/api/v1/uploads/%2E%2E/overview",
    ] {
        let (status, _) = send(&app, authed(test::TestRequest::get().uri(uri), &pc)).await;
        assert_ne!(status, StatusCode::OK, "{uri}");
    }
    // Nor does a computer token open a Mini App session, or the reverse.
    let (status, _) = send(
        &app,
        authed(test::TestRequest::get().uri("/api/v1/pc/session"), &anna),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A computer sees only the uploads it started.
    let (status, mine) = create(
        &app,
        &anna,
        json!({ "scope": "my", "name": "с-телефона.mov", "size": 10 }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let phone_upload = mine["id"].as_str().unwrap();
    let (status, created) = create(
        &app,
        &pc,
        json!({ "scope": "my", "name": "с-компьютера.mov", "size": 10 }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (_, listed) = send(
        &app,
        authed(test::TestRequest::get().uri("/api/v1/uploads"), &pc),
    )
    .await;
    let names: Vec<&str> = listed["uploads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["с-компьютера.mov"]);
    for req in [
        test::TestRequest::get().uri(&format!("/api/v1/uploads/{phone_upload}")),
        test::TestRequest::delete().uri(&format!("/api/v1/uploads/{phone_upload}")),
        test::TestRequest::post().uri(&format!("/api/v1/uploads/{phone_upload}/complete")),
    ] {
        let (status, _) = send(&app, authed(req, &pc)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    // The phone sees both.
    let (_, listed) = send(
        &app,
        authed(test::TestRequest::get().uri("/api/v1/uploads"), &anna),
    )
    .await;
    assert_eq!(listed["uploads"].as_array().unwrap().len(), 2);
}

#[actix_web::test]
async fn computers_are_disconnected_by_the_owner_revocation_and_unlinking() {
    let env = Env::new();
    let app = env.app().await;
    let anna = login(&app, ANNA).await;
    let bob = login(&app, BOB).await;
    let capacity = |token: &str| {
        authed(
            test::TestRequest::get().uri("/api/v1/uploads/capacity"),
            token,
        )
    };

    // «Отключить» in the Mini App; another user cannot do it.
    let (pc, id) = connect(&app, &anna).await;
    let (status, _) = send(
        &app,
        authed(
            test::TestRequest::delete().uri(&format!("/api/v1/computers/{id}")),
            &bob,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(send(&app, capacity(&pc)).await.0, StatusCode::OK);
    let (status, _) = send(
        &app,
        authed(
            test::TestRequest::delete().uri(&format!("/api/v1/computers/{id}")),
            &anna,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(send(&app, capacity(&pc)).await.0, StatusCode::UNAUTHORIZED);

    // «Отключить» on the computer itself.
    let (pc, _) = connect(&app, &anna).await;
    let (status, _) = send(
        &app,
        authed(test::TestRequest::delete().uri("/api/v1/pc/session"), &pc),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(send(&app, capacity(&pc)).await.0, StatusCode::UNAUTHORIZED);

    // «Завершить другие сеансы» ends computers too.
    let (pc, _) = connect(&app, &anna).await;
    let (status, _) = send(
        &app,
        authed(
            test::TestRequest::post().uri("/api/v1/sessions/revoke-all"),
            &anna,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(send(&app, capacity(&pc)).await.0, StatusCode::UNAUTHORIZED);

    // So does unlinking Telegram.
    let (pc, _) = connect(&app, &bob).await;
    assert_eq!(send(&app, capacity(&pc)).await.0, StatusCode::OK);
    let (status, _) = send(
        &app,
        authed(test::TestRequest::post().uri("/api/v1/tg/unlink"), &bob),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(send(&app, capacity(&pc)).await.0, StatusCode::UNAUTHORIZED);

    // A forged session id or a garbage token gets nothing.
    assert_eq!(
        send(&app, capacity("pc1.garbage.token")).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[actix_web::test]
async fn codes_are_single_use_validated_and_rate_limited() {
    let env = Env::new();
    let app = env.app().await;
    let anna = login(&app, ANNA).await;
    let bob = login(&app, BOB).await;
    let approve = |token: &str, code: &str| {
        authed(
            test::TestRequest::post().uri("/api/v1/computers/approve"),
            token,
        )
        .set_json(json!({ "code": code }))
    };

    let (_, code) = pair(&app).await;
    assert_eq!(send(&app, approve(&anna, &code)).await.0, StatusCode::OK);
    let (status, body) = send(&app, approve(&bob, &code)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("pair_not_found"))
    );
    let (status, body) = send(&app, approve(&bob, "не код")).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("bad_code"))
    );
    // 10 attempts per 10 minutes per Telegram account (bob has used 2).
    for _ in 0..8 {
        let (status, _) = send(&app, approve(&bob, "ZZZZ-ZZZZ")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (_, fresh) = pair(&app).await;
    let (status, body) = send(&app, approve(&bob, &fresh)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited"))
    );
    // Other accounts are not affected.
    assert_eq!(send(&app, approve(&anna, &fresh)).await.0, StatusCode::OK);
}

#[actix_web::test]
async fn uploads_that_do_not_fit_on_the_disk_are_refused_up_front() {
    if crate::storage::disk_usage(std::path::Path::new(".")).is_none() {
        return; // no free-space information on this platform
    }
    let env = Env::with(|config| config.upload_reserve = u64::MAX / 2);
    let app = env.app().await;
    let anna = login(&app, ANNA).await;
    let (status, capacity) = send(
        &app,
        authed(
            test::TestRequest::get().uri("/api/v1/uploads/capacity"),
            &anna,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(capacity["available_bytes"], 0);
    let (status, reply) = create(
        &app,
        &anna,
        json!({ "scope": "my", "name": "фильм.mkv", "size": 1024 }),
    )
    .await;
    assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE, "{reply}");
    assert_eq!(reply["code"], "no_space");
    assert_eq!(reply["available_bytes"], 0);
    assert!(reply["message"]
        .as_str()
        .unwrap()
        .contains("не хватает места"));
    // Nothing was staged.
    let staged = env.config.upload_dir.join(".staging/anna");
    assert!(fs::read_dir(staged).map_or(true, |mut d| d.next().is_none()));
}

#[actix_web::test]
async fn unfinished_uploads_count_as_promised_space() {
    let env = Env::new();
    let app = env.app().await;
    let anna = login(&app, ANNA).await;
    let available = |body: &Value| body["available_bytes"].as_u64();
    let (_, before) = send(
        &app,
        authed(
            test::TestRequest::get().uri("/api/v1/uploads/capacity"),
            &anna,
        ),
    )
    .await;
    let Some(before) = available(&before) else {
        return; // no free-space information on this platform
    };
    let (status, _) = create(
        &app,
        &anna,
        json!({ "scope": "my", "name": "большой.iso", "size": 50 * 1024 * 1024 }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, after) = send(
        &app,
        authed(
            test::TestRequest::get().uri("/api/v1/uploads/capacity"),
            &anna,
        ),
    )
    .await;
    let after = available(&after).unwrap();
    // 50 MiB promised; other processes may change the disk a little meanwhile.
    let promised = before.saturating_sub(after);
    assert!(
        (45 * 1024 * 1024..=55 * 1024 * 1024).contains(&promised),
        "promised {promised}"
    );
}

async fn put<S>(app: &S, token: &str, id: &str, offset: usize, data: &[u8]) -> StatusCode
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    send(
        app,
        authed(
            test::TestRequest::put().uri(&format!("/api/v1/uploads/{id}")),
            token,
        )
        .insert_header(("Upload-Offset", offset.to_string()))
        .set_payload(data.to_vec()),
    )
    .await
    .0
}

async fn staged<S>(app: &S, token: &str) -> Vec<Value>
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let (status, body) = send(
        app,
        authed(test::TestRequest::get().uri("/api/v1/uploads"), token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["uploads"].as_array().unwrap().clone()
}

#[actix_web::test]
async fn the_same_browser_resumes_its_uploads_after_reconnecting() {
    let env = Env::new();
    let app = env.app().await;
    let anna = login(&app, ANNA).await;
    let device = "Browser1".repeat(4);
    let (first, first_session) = connect_as(&app, &anna, &device).await;
    let (status, created) = create(
        &app,
        &first,
        json!({ "scope": "my", "name": "фильм.mkv", "size": 20 }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(put(&app, &first, &id, 0, &[1u8; 10]).await, StatusCode::OK);

    // The phone disconnects the computer; the browser pairs again.
    let (status, _) = send(
        &app,
        authed(
            test::TestRequest::delete().uri(&format!("/api/v1/computers/{first_session}")),
            &anna,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        put(&app, &first, &id, 10, &[2u8; 10]).await,
        StatusCode::UNAUTHORIZED
    );
    let (second, _) = connect_as(&app, &anna, &device).await;
    let list = staged(&app, &second).await;
    assert_eq!(list.len(), 1);
    assert_eq!(
        (list[0]["id"].as_str(), list[0]["offset"].as_u64()),
        (Some(id.as_str()), Some(10))
    );
    assert_eq!(
        put(&app, &second, &id, 10, &[2u8; 10]).await,
        StatusCode::OK
    );

    // Another browser of the same account sees none of it.
    let (other, _) = connect_as(&app, &anna, &"Browser2".repeat(4)).await;
    assert!(staged(&app, &other).await.is_empty());
    assert_eq!(
        put(&app, &other, &id, 20, &[3u8; 1]).await,
        StatusCode::NOT_FOUND
    );

    let (status, done) = send(
        &app,
        authed(
            test::TestRequest::post().uri(&format!("/api/v1/uploads/{id}/complete")),
            &second,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["item"]["category"], "Видео");
    let saved = fs::read(env.config.upload_dir.join("home/anna/Видео/фильм.mkv")).unwrap();
    assert_eq!(saved, [[1u8; 10], [2u8; 10]].concat());
}
