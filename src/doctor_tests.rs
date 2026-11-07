//! Authenticated doctor bridge on a real Actix service, using synthetic data.
use crate::tg_auth::test_support::{init_data, user_json};
use crate::{config::AppConfig, doctor::DoctorJobs, tokens::TokenKeys, users::UserStore};
use actix_web::{
    dev::{Service, ServiceResponse},
    http::{header, StatusCode},
    test, web, App,
};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

const SERVICE: &str = "doctor-synthetic-service-token-12345678";
const ALICE: i64 = 101;
const BOB: i64 = 202;

struct Env {
    root: PathBuf,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    keys: web::Data<TokenKeys>,
    jobs: web::Data<DoctorJobs>,
}
impl Env {
    fn new() -> Self {
        let root = crate::storage::test_directory("doctor");
        let mut config = AppConfig::for_tests(root.join("uploads"));
        config.internal_api_token = Some(SERVICE.into());
        let store = UserStore::load(root.join("users.json")).unwrap();
        for (name, id) in [("alice", ALICE), ("bob", BOB)] {
            store.add_user(name, "synthetic-test-password").unwrap();
            store.link_telegram(name, "admin", id).unwrap();
        }
        let keys = TokenKeys::derive(&config.secret);
        Self {
            root,
            config: web::Data::new(config),
            store: web::Data::new(store),
            keys: web::Data::new(keys),
            jobs: web::Data::new(DoctorJobs::default()),
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
                .configure(crate::api::configure)
                .configure(crate::internal::configure),
        )
        .await
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn signed(id: i64) -> String {
    let user = user_json(id, "test");
    let now = crate::reply::now_secs().to_string();
    init_data(&[("auth_date", &now), ("user", &user)])
}
fn bearer(req: test::TestRequest, token: &str) -> test::TestRequest {
    req.insert_header((header::AUTHORIZATION, format!("Bearer {token}")))
}
async fn send<S>(app: &S, req: test::TestRequest) -> (StatusCode, Value)
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let response = test::call_service(app, req.to_request()).await;
    let status = response.status();
    let body = test::read_body(response).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}
async fn login<S>(app: &S, id: i64) -> String
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let (status, body) = send(
        app,
        test::TestRequest::post()
            .uri("/api/v1/tg/session")
            .set_json(json!({"init_data": signed(id)})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["access_token"].as_str().unwrap().into()
}
async fn heartbeat<S>(app: &S)
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let (status, _) = send(
        app,
        bearer(
            test::TestRequest::get().uri("/internal/v1/doctor/jobs?wait=0"),
            SERVICE,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
fn submit(token: &str, id: &str, operation: &str, payload: Value) -> test::TestRequest {
    bearer(
        test::TestRequest::post()
            .uri("/api/v1/doctor/jobs")
            .set_json(json!({"request_id": id, "operation": operation, "payload": payload})),
        token,
    )
}

#[actix_web::test]
async fn bridge_requires_both_auth_boundaries_and_online_worker() {
    let env = Env::new();
    let app = env.app().await;
    let (status, _) = send(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/doctor/jobs")
            .set_json(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &app,
        test::TestRequest::get().uri("/internal/v1/doctor/jobs"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let token = login(&app, ALICE).await;
    let (status, body) = send(
        &app,
        submit(&token, "status-request-1", "status", json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "doctor_offline");
}

#[actix_web::test]
async fn jobs_are_private_idempotent_and_actor_is_from_verified_session() {
    let env = Env::new();
    let app = env.app().await;
    heartbeat(&app).await;
    let alice = login(&app, ALICE).await;
    let bob = login(&app, BOB).await;
    let (status, first) = send(
        &app,
        submit(
            &alice,
            "request-0001",
            "create",
            json!({"telegram_id": BOB}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, same) = send(&app, submit(&alice, "request-0001", "create", json!({}))).await;
    assert_eq!(first["job"], same["job"]);
    let id = first["job"].as_str().unwrap();
    let (status, _) = send(
        &app,
        bearer(
            test::TestRequest::get().uri(&format!("/api/v1/doctor/jobs/{id}")),
            &bob,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, taken) = send(
        &app,
        bearer(
            test::TestRequest::get().uri("/internal/v1/doctor/jobs?wait=0"),
            SERVICE,
        ),
    )
    .await;
    assert_eq!(taken["jobs"][0]["telegram_id"], ALICE);
    assert_eq!(taken["jobs"].as_array().unwrap().len(), 1);
    let (_, empty) = send(
        &app,
        bearer(
            test::TestRequest::get().uri("/internal/v1/doctor/jobs?wait=0"),
            SERVICE,
        ),
    )
    .await;
    assert_eq!(empty["jobs"], json!([]));
    let done_uri = format!("/internal/v1/doctor/jobs/{id}/done");
    let (status, _) = send(
        &app,
        bearer(
            test::TestRequest::post()
                .uri(&done_uri)
                .set_json(json!({"ok": true, "data": {"id": "w1234567"}})),
            &alice,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &app,
        bearer(
            test::TestRequest::post()
                .uri(&done_uri)
                .set_json(json!({"ok": true, "data": {"id": "w1234567"}})),
            SERVICE,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, done) = send(
        &app,
        bearer(
            test::TestRequest::get().uri(&format!("/api/v1/doctor/jobs/{id}")),
            &alice,
        ),
    )
    .await;
    assert_eq!(done["state"], "done");
    assert_eq!(done["result"]["data"]["id"], "w1234567");
}

#[actix_web::test]
async fn server_import_resolves_only_own_files_and_checks_size_and_symlinks() {
    let env = Env::new();
    let app = env.app().await;
    heartbeat(&app).await;
    let alice = login(&app, ALICE).await;
    let bob = login(&app, BOB).await;
    let folder = env.config.upload_dir.join("home/alice/Документы");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("жалоба.docx"), b"synthetic-docx").unwrap();
    let source = crate::storage::encode_id(
        crate::storage::Zone::My,
        "Документы",
        &crate::paths::RelPath::default(),
        Some("жалоба.docx"),
    );
    let (status, _) = send(
        &app,
        submit(
            &bob,
            "import-other-1",
            "review",
            json!({"source_id": source}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &app,
        submit(
            &alice,
            "import-own-01",
            "review",
            json!({"source_id": source, "role": "complaint"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, job) = send(
        &app,
        bearer(
            test::TestRequest::get().uri("/internal/v1/doctor/jobs?wait=0"),
            SERVICE,
        ),
    )
    .await;
    assert!(job["jobs"][0]["payload"]["source_id"].is_null());
    assert_eq!(job["jobs"][0]["payload"]["content"], "c3ludGhldGljLWRvY3g=");
    fs::File::create(folder.join("large.docx"))
        .unwrap()
        .set_len(5 * 1024 * 1024 + 1)
        .unwrap();
    let large = crate::storage::encode_id(
        crate::storage::Zone::My,
        "Документы",
        &crate::paths::RelPath::default(),
        Some("large.docx"),
    );
    let (status, _) = send(
        &app,
        submit(
            &alice,
            "import-large1",
            "review",
            json!({"source_id": large}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(folder.join("жалоба.docx"), folder.join("link.docx")).unwrap();
        let link = crate::storage::encode_id(
            crate::storage::Zone::My,
            "Документы",
            &crate::paths::RelPath::default(),
            Some("link.docx"),
        );
        let (status, _) = send(
            &app,
            submit(
                &alice,
                "import-link01",
                "review",
                json!({"source_id": link}),
            ),
        )
        .await;
        assert_ne!(status, StatusCode::OK);
    }
}

#[actix_web::test]
async fn invalid_operations_and_excess_jobs_are_rejected() {
    let env = Env::new();
    let app = env.app().await;
    heartbeat(&app).await;
    let token = login(&app, ALICE).await;
    for (id, op) in [("bad", "create"), ("valid-request-1", "../../read")] {
        let (status, _) = send(&app, submit(&token, id, op, json!({}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    for n in 0..4 {
        let (status, _) = send(
            &app,
            submit(&token, &format!("request-{n:04}"), "list", json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _) = send(&app, submit(&token, "request-9999", "list", json!({}))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

/// Opt-in local browser harness. The test signing key and synthetic accounts
/// exist only under cfg(test); the production binary keeps Telegram verification.
#[actix_web::test]
#[ignore = "local browser verification, see DOCTOR_WORKSPACE.md"]
async fn browser_harness() {
    let runtime = std::env::var("DOCTOR_UI_RUNTIME").expect("DOCTOR_UI_RUNTIME path");
    let env = Env::new();
    let config = env.config.clone();
    let store = env.store.clone();
    let keys = env.keys.clone();
    let jobs = env.jobs.clone();
    let server = actix_web::HttpServer::new(move || {
        App::new()
            .app_data(config.clone())
            .app_data(store.clone())
            .app_data(keys.clone())
            .app_data(jobs.clone())
            .app_data(web::Data::new(crate::api::link_limiter()))
            .app_data(web::Data::new(crate::uploads::UploadLocks::default()))
            .app_data(web::Data::new(crate::exchange::Events::default()))
            .app_data(web::Data::new(crate::chat::ChatJobs::default()))
            .configure(crate::api::configure)
            .configure(crate::internal::configure)
            .configure(crate::miniapp::configure)
    })
    .workers(1)
    .bind(("127.0.0.1", 0))
    .unwrap();
    let port = server.addrs()[0].port();
    fs::write(
        runtime,
        json!({"port": port, "init_data": signed(ALICE),
        "uploads": env.config.upload_dir, "user_id": ALICE})
        .to_string(),
    )
    .unwrap();
    server.run().await.unwrap();
}
