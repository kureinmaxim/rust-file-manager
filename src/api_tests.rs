//! Integration tests of the Mini App API on a real actix service with a
//! temporary upload directory and initData signed by the test key.

use std::fs;
use std::path::PathBuf;

use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{header, StatusCode};
use actix_web::{test, web, App};
use serde_json::{json, Value};

use crate::api::{self, link_limiter};
use crate::config::AppConfig;
use crate::tg_auth::test_support::{init_data, user_json};
use crate::tokens::TokenKeys;
use crate::uploads::{cleanup_staging, UploadLocks};
use crate::users::UserStore;

const ANNA: i64 = 101;
const BOB: i64 = 202;
const STRANGER: i64 = 909;

struct Env {
    dir: PathBuf,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    keys: web::Data<TokenKeys>,
    limiter: web::Data<crate::ratelimit::RateLimiter>,
    locks: web::Data<UploadLocks>,
}

impl Env {
    fn new() -> Self {
        let dir = crate::storage::test_directory("api");
        let config = AppConfig::for_tests(dir.join("uploads"));
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
            limiter: web::Data::new(link_limiter()),
            locks: web::Data::new(UploadLocks::default()),
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
                .app_data(self.limiter.clone())
                .app_data(self.locks.clone())
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

fn now() -> String {
    crate::reply::now_secs().to_string()
}

fn signed(tg_id: i64, username: &str, start_param: Option<&str>) -> String {
    let user = user_json(tg_id, username);
    let now = now();
    let mut fields = vec![("auth_date", now.as_str()), ("user", user.as_str())];
    if let Some(p) = start_param {
        fields.push(("start_param", p));
    }
    init_data(&fields)
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

async fn login<S>(app: &S, tg_id: i64) -> String
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let (status, body) = send(
        app,
        test::TestRequest::post()
            .uri("/api/v1/tg/session")
            .set_json(json!({ "init_data": signed(tg_id, "u", None) })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["access_token"].as_str().unwrap().to_string()
}

fn authed(req: test::TestRequest, token: &str) -> test::TestRequest {
    req.insert_header((header::AUTHORIZATION, format!("Bearer {token}")))
}

/// Upload `data` through the chunked protocol (chunk size 1 MiB in tests).
async fn upload<S>(app: &S, token: &str, body: Value, data: &[u8]) -> Value
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let mut body = body;
    body["size"] = json!(data.len());
    let (status, created) = send(
        app,
        authed(
            test::TestRequest::post()
                .uri("/api/v1/uploads")
                .set_json(&body),
            token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    let chunk = created["chunk_size"].as_u64().unwrap() as usize;
    let mut offset = 0usize;
    for part in data.chunks(chunk) {
        let (status, reply) = send(
            app,
            authed(
                test::TestRequest::put().uri(&format!("/api/v1/uploads/{id}")),
                token,
            )
            .insert_header(("Upload-Offset", offset.to_string()))
            .set_payload(part.to_vec()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        offset += part.len();
        assert_eq!(reply["offset"], offset);
    }
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

async fn list<S>(app: &S, token: &str, query: &str) -> Vec<Value>
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let (status, body) = send(
        app,
        authed(
            test::TestRequest::get().uri(&format!("/api/v1/files?{query}")),
            token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"].as_array().unwrap().clone()
}

fn q(value: &str) -> String {
    crate::storage::encode_url_segment(value)
}

#[actix_web::test]
async fn session_requires_valid_linked_init_data() {
    let env = Env::new();
    let app = env.app().await;

    let (status, body) = send(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/tg/session")
            .set_json(json!({ "init_data": signed(STRANGER, "Stranger_1", None) })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "not_linked");
    assert_eq!(body["suggested_username"], "stranger_1");
    assert_eq!(body["invite_valid"], false);

    let tampered = signed(ANNA, "u", None).replace("101", "202");
    let (status, body) = send(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/tg/session")
            .set_json(json!({ "init_data": tampered })),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("bad_init_data"))
    );

    let token = login(&app, ANNA).await;
    let (status, me) = send(
        &app,
        authed(test::TestRequest::get().uri("/api/v1/me"), &token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["user"]["username"], "anna");
    assert_eq!(me["user"]["is_admin"], false);
}

#[actix_web::test]
async fn bearer_is_required_and_revocable() {
    let env = Env::new();
    let app = env.app().await;
    for req in [
        test::TestRequest::get().uri("/api/v1/me"),
        authed(
            test::TestRequest::get().uri("/api/v1/me"),
            "v1.forged.token",
        ),
    ] {
        let (status, body) = send(&app, req).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::UNAUTHORIZED, Some("unauthorized"))
        );
    }

    let token = login(&app, ANNA).await;
    let (status, _) = send(
        &app,
        authed(
            test::TestRequest::post().uri("/api/v1/sessions/revoke-all"),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &app,
        authed(test::TestRequest::get().uri("/api/v1/me"), &token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "revoked token must stop working"
    );

    let bob = login(&app, BOB).await;
    env.store.remove_user("bob").unwrap();
    let (status, _) = send(
        &app,
        authed(test::TestRequest::get().uri("/api/v1/me"), &bob),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "deleted user must lose access"
    );
}

#[actix_web::test]
async fn link_with_password_and_rate_limit() {
    let env = Env::new();
    let app = env.app().await;
    env.store.add_user("maria", "maria-password").unwrap();
    let attempt =
        |password: &str| {
            test::TestRequest::post().uri("/api/v1/tg/link").set_json(json!({
            "init_data": signed(STRANGER, "m", None), "username": "Maria", "password": password,
        }))
        };
    let (status, body) = send(&app, attempt("wrong")).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("bad_credentials"))
    );
    let (status, body) = send(&app, attempt("maria-password")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["username"], "maria");
    assert_eq!(
        env.store
            .account_by_telegram(STRANGER, "admin")
            .unwrap()
            .username,
        "maria"
    );

    // The admin links with the env password.
    let admin_link = test::TestRequest::post()
        .uri("/api/v1/tg/link")
        .set_json(json!({
            "init_data": signed(555, "boss", None), "username": "admin", "password": "admin-pass",
        }));
    let (status, body) = send(&app, admin_link).await;
    assert_eq!(
        (status, body["user"]["is_admin"].as_bool()),
        (StatusCode::OK, Some(true))
    );

    // 5 attempts per Telegram account per window, whatever the outcome.
    for _ in 0..3 {
        send(&app, attempt("wrong")).await;
    }
    let (status, body) = send(&app, attempt("maria-password")).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited"))
    );
}

#[actix_web::test]
async fn register_with_invite_from_start_param() {
    let env = Env::new();
    let app = env.app().await;
    let invite = env.store.create_invite("admin").unwrap();
    let param = format!("inv_{}", invite.token);

    let (status, body) = send(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/tg/session")
            .set_json(json!({ "init_data": signed(STRANGER, "newbie", Some(&param)) })),
    )
    .await;
    assert_eq!(
        (status, body["invite_valid"].as_bool()),
        (StatusCode::FORBIDDEN, Some(true))
    );

    let register = |tg: i64, name: &str| {
        test::TestRequest::post()
            .uri("/api/v1/tg/register")
            .set_json(json!({
                "init_data": signed(tg, "x", Some(&param)), "username": name,
            }))
    };
    let (status, body) = send(&app, register(STRANGER, "anna")).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("username_taken"))
    );
    let (status, body) = send(&app, register(STRANGER, "Newbie")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["username"], "newbie");
    assert!(!env.store.invite_valid(&invite.token));
    assert!(
        !env.store.verify_password("newbie", ""),
        "Telegram accounts have no password"
    );

    let (status, body) = send(&app, register(808, "another")).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::GONE, Some("invite_invalid"))
    );
}

#[actix_web::test]
async fn chunked_upload_into_subfolders_and_listing() {
    let env = Env::new();
    let app = env.app().await;
    let token = login(&app, ANNA).await;

    // 2.5 chunks; category derived from the extension, folders created on completion.
    let data: Vec<u8> = (0..(2 * 1024 * 1024 + 512 * 1024))
        .map(|i| (i % 251) as u8)
        .collect();
    let item = upload(
        &app,
        &token,
        json!({ "scope": "my", "path": "Ремонт кухни/Чеки", "name": "чек #1.pdf", "category": "Документы" }),
        &data,
    )
    .await;
    assert_eq!(item["type"], "file");
    assert_eq!(item["category"], "Документы");
    assert_eq!(item["path"], "Ремонт кухни/Чеки");
    assert_eq!(item["kind"], "pdf");
    let on_disk = env
        .config
        .upload_dir
        .join("home/anna/Документы/Ремонт кухни/Чеки/чек #1.pdf");
    assert_eq!(fs::read(&on_disk).unwrap(), data);

    let photo = upload(
        &app,
        &token,
        json!({ "scope": "my", "name": "IMG_1.JPG" }),
        b"jpeg",
    )
    .await;
    assert_eq!(photo["category"], "Фото");

    // Same name again: never overwritten.
    let again = upload(
        &app,
        &token,
        json!({ "scope": "my", "name": "IMG_1.JPG" }),
        b"second",
    )
    .await;
    assert_eq!(again["name"], "IMG_1(1).JPG");

    let root = list(
        &app,
        &token,
        &format!("scope=my&category={}", q("Документы")),
    )
    .await;
    assert_eq!(root.len(), 1);
    assert_eq!(
        (root[0]["type"].as_str(), root[0]["name"].as_str()),
        (Some("folder"), Some("Ремонт кухни"))
    );
    assert_eq!(root[0]["files"], 1);
    assert_eq!(root[0]["folders"], 1);

    let inner = list(
        &app,
        &token,
        &format!(
            "scope=my&category={}&path={}",
            q("Документы"),
            q("Ремонт кухни/Чеки")
        ),
    )
    .await;
    assert_eq!(inner.len(), 1);
    assert_eq!(inner[0]["name"], "чек #1.pdf");

    let all = list(&app, &token, "scope=my").await;
    assert_eq!(all.len(), 3, "folder first, then two photos: {all:?}");
    assert_eq!(all[0]["type"], "folder");

    let found = list(&app, &token, &format!("scope=my&q={}", q("ЧЕК"))).await;
    let names: Vec<_> = found.iter().map(|i| i["name"].as_str().unwrap()).collect();
    assert!(
        names.contains(&"Чеки") && names.contains(&"чек #1.pdf"),
        "{names:?}"
    );

    let (status, overview) = send(
        &app,
        authed(test::TestRequest::get().uri("/api/v1/overview"), &token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(overview["zones"][0]["files"], 3);
    assert_eq!(overview["recent"].as_array().unwrap().len(), 3);
    assert!(
        overview["disk"].is_null(),
        "disk usage is for the admin only"
    );
}

#[actix_web::test]
async fn upload_protocol_rejects_wrong_offsets_and_oversize() {
    let env = Env::new();
    let app = env.app().await;
    let token = login(&app, ANNA).await;
    let create = |size: u64| {
        authed(test::TestRequest::post().uri("/api/v1/uploads"), &token)
            .set_json(json!({ "scope": "shared", "name": "a.bin", "size": size }))
    };
    let (status, body) = send(&app, create(1 << 40)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, Some("too_large"))
    );

    let (_, created) = send(&app, create(10)).await;
    let id = created["id"].as_str().unwrap().to_string();
    let put = |offset: u64, data: &[u8]| {
        authed(
            test::TestRequest::put().uri(&format!("/api/v1/uploads/{id}")),
            &token,
        )
        .insert_header(("Upload-Offset", offset.to_string()))
        .set_payload(data.to_vec())
    };
    let (status, body) = send(&app, put(3, b"abc")).await;
    assert_eq!(
        (status, body["offset"].as_u64()),
        (StatusCode::CONFLICT, Some(0))
    );
    let (status, _) = send(&app, put(0, b"abcdef")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(&app, put(6, b"too-much-data")).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, Some("chunk_too_large"))
    );
    // Rolled back to the confirmed offset.
    let (_, status_body) = send(
        &app,
        authed(
            test::TestRequest::get().uri(&format!("/api/v1/uploads/{id}")),
            &token,
        ),
    )
    .await;
    assert_eq!(status_body["offset"], 6);
    let (status, body) = send(
        &app,
        authed(
            test::TestRequest::post().uri(&format!("/api/v1/uploads/{id}/complete")),
            &token,
        ),
    )
    .await;
    assert_eq!(
        (status, body["offset"].as_u64()),
        (StatusCode::CONFLICT, Some(6))
    );

    // Another user cannot see or touch this upload.
    let bob = login(&app, BOB).await;
    let (status, _) = send(
        &app,
        authed(
            test::TestRequest::get().uri(&format!("/api/v1/uploads/{id}")),
            &bob,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, pending) = send(
        &app,
        authed(test::TestRequest::get().uri("/api/v1/uploads"), &token),
    )
    .await;
    assert_eq!(pending["uploads"].as_array().unwrap().len(), 1);

    let (status, _) = send(
        &app,
        authed(
            test::TestRequest::delete().uri(&format!("/api/v1/uploads/{id}")),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, pending) = send(
        &app,
        authed(test::TestRequest::get().uri("/api/v1/uploads"), &token),
    )
    .await;
    assert!(pending["uploads"].as_array().unwrap().is_empty());
}

#[actix_web::test]
async fn private_zone_is_isolated_and_shared_zone_is_common() {
    let env = Env::new();
    let app = env.app().await;
    let anna = login(&app, ANNA).await;
    let bob = login(&app, BOB).await;
    let secret = upload(
        &app,
        &anna,
        json!({ "scope": "my", "name": "secret.txt" }),
        b"anna only",
    )
    .await;
    let common = upload(
        &app,
        &anna,
        json!({ "scope": "shared", "name": "common.txt" }),
        b"for all",
    )
    .await;

    // Bob's "my" is his own home: Anna's id resolves inside Bob's zone and finds nothing.
    let id = secret["id"].as_str().unwrap();
    for req in [
        test::TestRequest::get().uri(&format!("/api/v1/files/{id}")),
        test::TestRequest::post()
            .uri(&format!("/api/v1/files/{id}/link"))
            .set_json(json!({})),
        test::TestRequest::delete().uri(&format!("/api/v1/files/{id}")),
    ] {
        let (status, _) = send(&app, authed(req, &bob)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    assert!(env
        .config
        .upload_dir
        .join("home/anna/Документы/secret.txt")
        .exists());
    assert!(list(&app, &bob, "scope=my").await.is_empty());

    let shared = list(&app, &bob, "scope=shared").await;
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0]["id"], common["id"]);
}

#[actix_web::test]
async fn traversal_and_symlinks_are_refused() {
    let env = Env::new();
    let app = env.app().await;
    let token = login(&app, ANNA).await;
    for query in [
        format!("scope=my&category={}&path=..", q("Документы")),
        format!(
            "scope=my&category={}&path={}",
            q("Документы"),
            q("a/../../..")
        ),
        "scope=my&category=..&path=".to_string(),
        "scope=root".to_string(),
        "scope=my&path=x".to_string(),
    ] {
        let (status, _) = send(
            &app,
            authed(
                test::TestRequest::get().uri(&format!("/api/v1/files?{query}")),
                &token,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
    }
    let (status, _) = send(
        &app,
        authed(test::TestRequest::post().uri("/api/v1/folders"), &token).set_json(
            json!({ "scope": "my", "category": "Фото", "path": "", "name": "../escape" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(
        &app,
        authed(test::TestRequest::post().uri("/api/v1/uploads"), &token)
            .set_json(json!({ "scope": "my", "name": "../../x.txt", "size": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    #[cfg(unix)]
    {
        let photos = env.config.upload_dir.join("home/anna/Фото");
        fs::create_dir_all(&photos).unwrap();
        std::os::unix::fs::symlink("/etc", photos.join("etc")).unwrap();
        // Hidden from listings…
        let items = list(&app, &token, &format!("scope=my&category={}", q("Фото"))).await;
        assert!(items.is_empty(), "{items:?}");
        // …and refused when addressed directly.
        let (status, body) = send(
            &app,
            authed(
                test::TestRequest::get().uri(&format!(
                    "/api/v1/files?scope=my&category={}&path=etc",
                    q("Фото")
                )),
                &token,
            ),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("bad_request"))
        );
        let (status, _) = send(
            &app,
            authed(test::TestRequest::post().uri("/api/v1/uploads"), &token)
                .set_json(json!({ "scope": "my", "category": "Фото", "path": "etc", "name": "passwd", "size": 1 })),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}

#[actix_web::test]
async fn rename_create_and_delete() {
    let env = Env::new();
    let app = env.app().await;
    let token = login(&app, ANNA).await;
    let a = upload(
        &app,
        &token,
        json!({ "scope": "my", "name": "a.txt" }),
        b"A",
    )
    .await;
    upload(
        &app,
        &token,
        json!({ "scope": "my", "name": "b.txt" }),
        b"B",
    )
    .await;

    let rename = |id: &str, name: &str| {
        authed(
            test::TestRequest::post().uri(&format!("/api/v1/files/{id}/rename")),
            &token,
        )
        .set_json(json!({ "new_name": name }))
    };
    let (status, body) = send(&app, rename(a["id"].as_str().unwrap(), "b.txt")).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("exists"))
    );
    assert_eq!(
        fs::read(env.config.upload_dir.join("home/anna/Документы/b.txt")).unwrap(),
        b"B"
    );
    let (status, body) = send(&app, rename(a["id"].as_str().unwrap(), "c.txt")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["item"]["name"], "c.txt");

    let folder = |name: &str, path: &str| {
        authed(test::TestRequest::post().uri("/api/v1/folders"), &token)
            .set_json(json!({ "scope": "my", "category": "Документы", "path": path, "name": name }))
    };
    let (status, created) = send(&app, folder("Ремонт", "")).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let (status, _) = send(&app, folder("Ремонт", "")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(&app, folder("Чеки", "Нет такой")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, sub) = send(&app, folder("Чеки", "Ремонт")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sub["item"]["path"], "Ремонт");

    let folder_id = created["item"]["id"].as_str().unwrap().to_string();
    let (status, body) = send(
        &app,
        authed(
            test::TestRequest::delete().uri(&format!("/api/v1/files/{folder_id}")),
            &token,
        ),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("not_empty"))
    );
    let (status, renamed) = send(&app, rename(&folder_id, "Ремонт кухни")).await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert!(env
        .config
        .upload_dir
        .join("home/anna/Документы/Ремонт кухни/Чеки")
        .is_dir());
    let (status, _) = send(
        &app,
        rename(renamed["item"]["id"].as_str().unwrap(), ".hidden"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[actix_web::test]
async fn signed_links_download_ranges_and_expire_on_revoke() {
    let env = Env::new();
    let app = env.app().await;
    let token = login(&app, ANNA).await;
    let file = upload(
        &app,
        &token,
        json!({ "scope": "my", "name": "photo.jpg" }),
        b"0123456789",
    )
    .await;
    let svg = upload(
        &app,
        &token,
        json!({ "scope": "my", "name": "x.svg" }),
        b"<svg onload=alert(1)/>",
    )
    .await;
    let link = |id: &str, purpose: &str| {
        authed(
            test::TestRequest::post().uri(&format!("/api/v1/files/{id}/link")),
            &token,
        )
        .set_json(json!({ "purpose": purpose }))
    };

    let (_, l) = send(&app, link(file["id"].as_str().unwrap(), "inline")).await;
    let url = l["url"].as_str().unwrap().to_string();
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&url)
            .insert_header((header::RANGE, "bytes=2-5"))
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let headers = resp.headers().clone();
    assert!(headers
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("inline"));
    assert_eq!(
        headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
        "nosniff"
    );
    assert!(headers
        .get(header::CONTENT_SECURITY_POLICY)
        .unwrap()
        .to_str()
        .unwrap()
        .contains("sandbox"));
    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "https://web.telegram.org"
    );
    assert_eq!(test::read_body(resp).await.as_ref(), b"2345");

    // SVG is never inline, even when asked for.
    let (_, l) = send(&app, link(svg["id"].as_str().unwrap(), "inline")).await;
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(l["url"].as_str().unwrap())
            .to_request(),
    )
    .await;
    assert!(resp
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("attachment"));

    // Tampered and revoked links stop working.
    let (status, _) = send(&app, test::TestRequest::get().uri(&format!("{url}x"))).await;
    assert_eq!(status, StatusCode::GONE);
    env.store.bump_token_version("anna", "admin").unwrap();
    let (status, _) = send(&app, test::TestRequest::get().uri(&url)).await;
    assert_eq!(status, StatusCode::GONE);
}

#[actix_web::test]
async fn stale_staging_is_cleaned_up() {
    let dir = crate::storage::test_directory("staging");
    let user = dir.join(crate::storage::STAGING_DIR).join("anna");
    fs::create_dir_all(&user).unwrap();
    fs::write(user.join("x.part"), b"1").unwrap();
    assert_eq!(
        cleanup_staging(&dir, std::time::Duration::from_secs(3600)),
        0
    );
    assert_eq!(cleanup_staging(&dir, std::time::Duration::ZERO), 1);
    fs::remove_dir_all(dir).unwrap();
}
