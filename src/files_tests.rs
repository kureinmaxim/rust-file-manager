//! Web UI routes with subfolders: the same folder tree as the Mini App,
//! reached through the cookie session, on a temporary upload directory.

use std::fs;
use std::path::PathBuf;

use actix_session::storage::CookieSessionStore;
use actix_session::{Session, SessionMiddleware};
use actix_web::cookie::{Cookie, Key};
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::StatusCode;
use actix_web::middleware::from_fn;
use actix_web::{test, web, App, HttpResponse};
use serde_json::Value;

use crate::auth;
use crate::config::AppConfig;
use crate::files;
use crate::storage::encode_url_segment as enc;
use crate::users::UserStore;

struct Env {
    dir: PathBuf,
    config: web::Data<AppConfig>,
    store: web::Data<UserStore>,
    events: web::Data<crate::exchange::Events>,
}

impl Env {
    fn new() -> Self {
        let dir = crate::storage::test_directory("web");
        let config = AppConfig::for_tests(dir.join("uploads"));
        let store = UserStore::load(dir.join("users.json")).unwrap();
        store.add_telegram_user("anna", 101).unwrap();
        store.add_telegram_user("bob", 202).unwrap();
        Self {
            dir,
            config: web::Data::new(config),
            store: web::Data::new(store),
            events: web::Data::new(crate::exchange::Events::default()),
        }
    }

    fn uploads(&self) -> PathBuf {
        self.dir.join("uploads")
    }

    async fn app(
        &self,
    ) -> impl Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>
    {
        test::init_service(
            App::new()
                .app_data(self.config.clone())
                .app_data(self.store.clone())
                .app_data(web::Data::new(files::templates()))
                .app_data(self.events.clone())
                .wrap(
                    SessionMiddleware::builder(CookieSessionStore::default(), Key::from(&[9; 64]))
                        .cookie_secure(false)
                        .build(),
                )
                .route(
                    "/test-login/{user}",
                    web::get().to(|session: Session, user: web::Path<String>| async move {
                        auth::sign_in_for_tests(&session, &user, false);
                        HttpResponse::Ok().finish()
                    }),
                )
                .route(
                    "/test-login-admin",
                    web::get().to(|session: Session| async move {
                        auth::sign_in_for_tests(&session, "admin", true);
                        HttpResponse::Ok().finish()
                    }),
                )
                .service(
                    web::scope("/admin")
                        .wrap(from_fn(auth::require_admin))
                        .wrap(from_fn(auth::require_auth))
                        .service(crate::admin::delete_user),
                )
                .service(
                    web::scope("")
                        .wrap(from_fn(auth::require_auth))
                        .service(files::index)
                        .service(files::upload)
                        .service(files::delete_file)
                        .service(files::rename_file)
                        .service(files::move_file)
                        .service(files::download)
                        .service(files::create_folder)
                        .service(files::rename_folder)
                        .service(files::delete_folder)
                        .service(crate::exchange::web_upload)
                        .service(crate::exchange::web_download)
                        .service(crate::exchange::web_delete)
                        .service(crate::exchange::web_save),
                ),
        )
        .await
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

async fn sign_in(
    app: &impl Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
    user: &str,
) -> Cookie<'static> {
    let uri = if user == "admin" {
        "/test-login-admin".to_string()
    } else {
        format!("/test-login/{user}")
    };
    let resp = test::call_service(app, test::TestRequest::get().uri(&uri).to_request()).await;
    resp.response().cookies().next().unwrap().into_owned()
}

async fn call(
    app: &impl Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
    cookie: &Cookie<'static>,
    req: test::TestRequest,
) -> (StatusCode, Value) {
    let resp = test::call_service(
        app,
        req.cookie(cookie.clone())
            .insert_header(("Accept", "application/json"))
            .to_request(),
    )
    .await;
    let status = resp.status();
    let body = test::read_body(resp).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

fn query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn new_folder(scope: &str, category: &str, path: &str, name: &str) -> test::TestRequest {
    test::TestRequest::post().uri(&format!(
        "/folders/{scope}/{}?{}",
        enc(category),
        query(&[("path", path), ("name", name)])
    ))
}

fn upload(scope: &str, params: &[(&str, &str)], name: &str, body: &[u8]) -> test::TestRequest {
    let boundary = "rfm-test-boundary";
    let mut payload = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    payload.extend_from_slice(body);
    payload.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    test::TestRequest::post()
        .uri(&format!("/upload/{scope}?{}", query(params)))
        .insert_header((
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        ))
        .set_payload(payload)
}

#[actix_web::test]
async fn folders_in_the_web_ui_match_the_mini_app_tree() {
    let env = Env::new();
    let app = env.app().await;
    let anna = sign_in(&app, "anna").await;
    let docs = env.uploads().join("shared").join("Документы");

    let (status, _) = call(
        &app,
        &anna,
        new_folder("shared", "Документы", "", "Проекты"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        &anna,
        new_folder("shared", "Документы", "Проекты", "2026 #1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(docs.join("Проекты").join("2026 #1").is_dir());

    // Taken names, hidden names, slashes and traversal are refused.
    let (status, _) = call(
        &app,
        &anna,
        new_folder("shared", "Документы", "", "Проекты"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    for (path, name) in [
        ("", ".hidden"),
        ("", "a/b"),
        ("..", "x"),
        ("Проекты/../..", "x"),
    ] {
        let (status, body) = call(&app, &anna, new_folder("shared", "Документы", path, name)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path} {name}: {body}");
    }
    let (status, _) = call(&app, &anna, new_folder("shared", "Нет такой", "", "x")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Upload into the folder; a folder needs an explicit category and must exist.
    let folder = [("category", "Документы"), ("path", "Проекты/2026 #1")];
    let (status, body) = call(&app, &anna, upload("shared", &folder, "отчёт?.pdf", b"PDF")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        fs::read(docs.join("Проекты").join("2026 #1").join("отчёт?.pdf")).unwrap(),
        b"PDF"
    );
    let (status, _) = call(
        &app,
        &anna,
        upload("shared", &[("path", "Проекты")], "a.pdf", b"1"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let missing = [("category", "Документы"), ("path", "Нет")];
    let (status, _) = call(&app, &anna, upload("shared", &missing, "a.pdf", b"1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!docs.join("Нет").exists());
    // Same name again: a new file next to it, never an overwrite.
    let (status, _) = call(&app, &anna, upload("shared", &folder, "отчёт?.pdf", b"NEW")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        fs::read(docs.join("Проекты").join("2026 #1").join("отчёт?.pdf")).unwrap(),
        b"PDF"
    );
    assert!(docs
        .join("Проекты")
        .join("2026 #1")
        .join("отчёт?(1).pdf")
        .is_file());

    // The page lists the tree with per-segment-encoded links into folders.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(anna.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = String::from_utf8(test::read_body(resp).await.to_vec()).unwrap();
    assert!(
        html.contains(r#"data-path="Проекты/2026 #1""#),
        "folder in the tree"
    );
    let link = format!(
        "/uploads/shared/{}/{}?path={}",
        enc("Документы"),
        enc("отчёт?.pdf"),
        enc("Проекты/2026 #1")
    );
    // Handlebars escapes `=` in attributes as `&#x3D;`; browsers decode it back.
    assert!(
        html.contains(&format!(r#"href="{}""#, link.replace('=', "&#x3D;"))),
        "download link"
    );

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&link)
            .cookie(anna.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(test::read_body(resp).await.as_ref(), b"PDF");

    // Rename inside the folder refuses to overwrite.
    let file = |name: &str| {
        format!(
            "{}/{}?{}",
            enc("Документы"),
            enc(name),
            query(&[("path", "Проекты/2026 #1")])
        )
    };
    let (status, _) = call(
        &app,
        &anna,
        test::TestRequest::post().uri(&format!(
            "/rename/shared/{}&newName={}",
            file("отчёт?(1).pdf"),
            enc("отчёт?.pdf")
        )),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &app,
        &anna,
        test::TestRequest::post().uri(&format!(
            "/rename/shared/{}&newName={}",
            file("отчёт?(1).pdf"),
            enc("итог.pdf")
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        fs::read(docs.join("Проекты").join("2026 #1").join("итог.pdf")).unwrap(),
        b"NEW"
    );

    // Folders: a non-empty one stays, an empty one goes, renames never replace.
    let delete_folder = |path: &str| {
        test::TestRequest::delete().uri(&format!(
            "/folders/shared/{}?{}",
            enc("Документы"),
            query(&[("path", path)])
        ))
    };
    let (status, _) = call(&app, &anna, delete_folder("Проекты/2026 #1")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    for name in ["отчёт?.pdf", "итог.pdf"] {
        let (status, _) = call(
            &app,
            &anna,
            test::TestRequest::delete().uri(&format!("/delete/shared/{}", file(name))),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _) = call(&app, &anna, delete_folder("")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(&app, &anna, delete_folder("Проекты/2026 #1")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!docs.join("Проекты").join("2026 #1").exists());

    fs::create_dir_all(docs.join("Архив")).unwrap();
    let rename_folder = |path: &str, new_name: &str| {
        test::TestRequest::post().uri(&format!(
            "/folders/shared/{}/rename?{}",
            enc("Документы"),
            query(&[("path", path), ("newName", new_name)])
        ))
    };
    let (status, _) = call(&app, &anna, rename_folder("Проекты", "Архив")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(&app, &anna, rename_folder("Проекты", "Старое")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(docs.join("Старое").is_dir() && !docs.join("Проекты").exists());

    // Without `path` the old routes behave as before: category by extension.
    let (status, _) = call(&app, &anna, upload("shared", &[], "root.pdf", b"R")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(docs.join("root.pdf").is_file());
}

#[actix_web::test]
async fn private_folders_and_symlinks_stay_out_of_reach() {
    let env = Env::new();
    let app = env.app().await;
    let anna = sign_in(&app, "anna").await;
    let bob = sign_in(&app, "bob").await;

    let (status, _) = call(&app, &anna, new_folder("my", "Документы", "", "Секрет")).await;
    assert_eq!(status, StatusCode::OK);
    let secret = [("category", "Документы"), ("path", "Секрет")];
    let (status, _) = call(&app, &anna, upload("my", &secret, "x.pdf", b"S")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(env
        .uploads()
        .join("home/anna/Документы/Секрет/x.pdf")
        .is_file());

    // Bob's "my" is his own home: Anna's folder does not exist there.
    let url = format!(
        "/uploads/my/{}/x.pdf?{}",
        enc("Документы"),
        query(&[("path", "Секрет")])
    );
    let (status, _) = call(&app, &bob, test::TestRequest::get().uri(&url)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(bob.clone())
            .to_request(),
    )
    .await;
    let html = String::from_utf8(test::read_body(resp).await.to_vec()).unwrap();
    assert!(!html.contains("Секрет"));

    // A symlink planted inside a zone is neither listed nor followed.
    let outside = env.dir.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("leak.pdf"), b"L").unwrap();
    let docs = env.uploads().join("shared").join("Документы");
    fs::create_dir_all(&docs).unwrap();
    std::os::unix::fs::symlink(&outside, docs.join("link")).unwrap();
    let url = format!(
        "/uploads/shared/{}/leak.pdf?{}",
        enc("Документы"),
        query(&[("path", "link")])
    );
    let (status, _) = call(&app, &anna, test::TestRequest::get().uri(&url)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let leak = [("category", "Документы"), ("path", "link")];
    let (status, _) = call(&app, &anna, upload("shared", &leak, "in.pdf", b"I")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!outside.join("in.pdf").exists());
    let (status, _) = call(&app, &anna, new_folder("shared", "Документы", "link", "x")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!outside.join("x").exists());
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(anna.clone())
            .to_request(),
    )
    .await;
    let html = String::from_utf8(test::read_body(resp).await.to_vec()).unwrap();
    assert!(!html.contains("leak.pdf") && !html.contains(r#"data-path="link""#));

    // No session, no access.
    let resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri(&format!("/folders/shared/{}?name=x", enc("Документы")))
            .insert_header(("Accept", "application/json"))
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

fn move_file(
    scope: &str,
    category: &str,
    path: &str,
    name: &str,
    to: &[(&str, &str)],
) -> test::TestRequest {
    let mut params = vec![("path", path)];
    params.extend_from_slice(to);
    test::TestRequest::post().uri(&format!(
        "/move/{scope}/{}/{}?{}",
        enc(category),
        enc(name),
        query(&params)
    ))
}

#[actix_web::test]
async fn files_move_between_zones_and_folders_without_overwriting() {
    let env = Env::new();
    let app = env.app().await;
    let anna = sign_in(&app, "anna").await;
    let bob = sign_in(&app, "bob").await;
    let shared = env.uploads().join("shared").join("Документы");
    let mine = env.uploads().join("home").join("anna").join("Документы");
    let to_my = [
        ("toScope", "my"),
        ("toCategory", "Документы"),
        ("toPath", ""),
    ];

    // Shared → my: the file leaves the shared zone.
    let (status, _) = call(&app, &anna, upload("shared", &[], "акт №1.pdf", b"OLD")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &app,
        &anna,
        move_file("shared", "Документы", "", "акт №1.pdf", &to_my),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(fs::read(mine.join("акт №1.pdf")).unwrap(), b"OLD");
    assert!(!shared.join("акт №1.pdf").exists());

    // Back to shared, where a file with that name appeared meanwhile: suffix, no overwrite.
    let (status, _) = call(&app, &anna, upload("shared", &[], "акт №1.pdf", b"NEW")).await;
    assert_eq!(status, StatusCode::OK);
    let to_shared = [
        ("toScope", "shared"),
        ("toCategory", "Документы"),
        ("toPath", ""),
    ];
    let (status, body) = call(
        &app,
        &anna,
        move_file("my", "Документы", "", "акт №1.pdf", &to_shared),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["message"].as_str().unwrap().contains("акт №1(1).pdf"),
        "{body}"
    );
    assert_eq!(fs::read(shared.join("акт №1.pdf")).unwrap(), b"NEW");
    assert_eq!(fs::read(shared.join("акт №1(1).pdf")).unwrap(), b"OLD");

    // Into an existing folder of my zone; a missing folder or the same place is refused.
    let (status, _) = call(&app, &anna, new_folder("my", "Документы", "", "Архив")).await;
    assert_eq!(status, StatusCode::OK);
    let to_archive = [
        ("toScope", "my"),
        ("toCategory", "Документы"),
        ("toPath", "Архив"),
    ];
    let (status, _) = call(
        &app,
        &anna,
        move_file("shared", "Документы", "", "акт №1(1).pdf", &to_archive),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(mine.join("Архив").join("акт №1(1).pdf").is_file());
    let missing = [
        ("toScope", "my"),
        ("toCategory", "Документы"),
        ("toPath", "Нет"),
    ];
    let (status, _) = call(
        &app,
        &anna,
        move_file("shared", "Документы", "", "акт №1.pdf", &missing),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!mine.join("Нет").exists());
    let here = [
        ("toScope", "shared"),
        ("toCategory", "Документы"),
        ("toPath", ""),
    ];
    let (status, _) = call(
        &app,
        &anna,
        move_file("shared", "Документы", "", "акт №1.pdf", &here),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    for to in [
        [
            ("toScope", "home"),
            ("toCategory", "Документы"),
            ("toPath", ""),
        ],
        [
            ("toScope", "my"),
            ("toCategory", "../shared"),
            ("toPath", ""),
        ],
        [
            ("toScope", "my"),
            ("toCategory", "Документы"),
            ("toPath", "../../bob"),
        ],
    ] {
        let (status, body) = call(
            &app,
            &anna,
            move_file("shared", "Документы", "", "акт №1.pdf", &to),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{to:?}: {body}");
    }
    assert!(shared.join("акт №1.pdf").is_file());

    // Another category works; Bob cannot reach Anna's private files.
    let to_video = [("toScope", "my"), ("toCategory", "Видео"), ("toPath", "")];
    let (status, _) = call(
        &app,
        &anna,
        move_file("shared", "Документы", "", "акт №1.pdf", &to_video),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(env.uploads().join("home/anna/Видео/акт №1.pdf").is_file());
    let to_bob = [("toScope", "my"), ("toCategory", "Видео"), ("toPath", "")];
    let (status, _) = call(
        &app,
        &bob,
        move_file("my", "Документы", "Архив", "акт №1(1).pdf", &to_bob),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(mine.join("Архив").join("акт №1(1).pdf").is_file());

    // A planted symlink is not a destination.
    let outside = env.dir.join("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, mine.join("link")).unwrap();
    let to_link = [
        ("toScope", "my"),
        ("toCategory", "Документы"),
        ("toPath", "link"),
    ];
    let (status, _) = call(
        &app,
        &anna,
        move_file("my", "Документы", "Архив", "акт №1(1).pdf", &to_link),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
}

fn exchange_upload(owner: &str, name: &str, body: &[u8]) -> test::TestRequest {
    let mut req = upload("x", &[], name, body);
    req = req.uri(&format!("/exchange/{}/upload", enc(owner)));
    req
}

async fn page(
    app: &impl Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
    cookie: &Cookie<'static>,
) -> String {
    let resp = test::call_service(
        app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    String::from_utf8(test::read_body(resp).await.to_vec()).unwrap()
}

#[actix_web::test]
async fn exchange_is_private_to_the_admin_and_one_user() {
    use std::os::unix::fs::MetadataExt;

    let env = Env::new();
    let app = env.app().await;
    let admin = sign_in(&app, "admin").await;
    let anna = sign_in(&app, "anna").await;
    let bob = sign_in(&app, "bob").await;
    let exchange = env.uploads().join("exchange");

    // Admin → Anna.
    let (status, body) = call(
        &app,
        &admin,
        exchange_upload("anna", "договор #1.pdf", b"DOC"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = exchange.join("anna/from-admin/договор #1.pdf");
    assert_eq!(fs::read(&sent).unwrap(), b"DOC");
    let recorded = env.events.after(0, 10);
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        (
            recorded[0].owner.as_str(),
            recorded[0].name.as_str(),
            recorded[0].size
        ),
        ("anna", "договор #1.pdf", 3)
    );
    assert_eq!(recorded[0].inbox, crate::exchange::Inbox::FromAdmin);
    assert!(page(&app, &anna).await.contains("договор #1.pdf"));
    assert!(page(&app, &admin).await.contains("договор #1.pdf"));
    assert!(!page(&app, &bob).await.contains("договор #1.pdf"));

    // Bob reaches none of it: not by name, not by sending into it.
    let file = format!("/exchange/anna/from-admin/{}", enc("договор #1.pdf"));
    for req in [
        test::TestRequest::get().uri(&file),
        test::TestRequest::delete().uri(&file),
        test::TestRequest::post().uri(&format!("{file}/save")),
    ] {
        let (status, _) = call(&app, &bob, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, _) = call(&app, &bob, exchange_upload("anna", "x.pdf", b"X")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A user cannot open an "exchange" of the admin or of an unknown user either.
    for owner in ["admin", "zed"] {
        let (status, _) = call(&app, &anna, exchange_upload(owner, "x.pdf", b"X")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{owner}");
    }
    let (status, _) = call(&app, &admin, exchange_upload("zed", "x.pdf", b"X")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!exchange.join("zed").exists());
    for (inbox, name) in [
        ("other", "a.pdf"),
        ("from-admin", ".."),
        ("from-admin", " a.pdf"),
    ] {
        let (status, _) = call(
            &app,
            &anna,
            test::TestRequest::get().uri(&format!("/exchange/anna/{inbox}/{}", enc(name))),
        )
        .await;
        assert!(status.is_client_error(), "{inbox}/{name}: {status}");
    }

    // Anna downloads it and saves a copy: a hard link, the original stays.
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&file)
            .cookie(anna.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(test::read_body(resp).await.as_ref(), b"DOC");
    for expected in ["договор #1.pdf", "договор #1(1).pdf"] {
        let (status, body) = call(
            &app,
            &anna,
            test::TestRequest::post().uri(&format!("{file}/save")),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["name"], expected);
        let copy = env.uploads().join("home/anna/Документы").join(expected);
        assert_eq!(fs::read(&copy).unwrap(), b"DOC");
        assert_eq!(
            fs::metadata(&copy).unwrap().ino(),
            fs::metadata(&sent).unwrap().ino()
        );
    }
    assert!(sent.is_file());

    // Anna → admin; the admin saves it into the admin's own files.
    let (status, _) = call(&app, &anna, exchange_upload("anna", "отчёт.xlsx", b"XLS")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(exchange.join("anna/from-user/отчёт.xlsx").is_file());
    let report = format!("/exchange/anna/from-user/{}", enc("отчёт.xlsx"));
    let (status, _) = call(
        &app,
        &admin,
        test::TestRequest::post().uri(&format!("{report}/save")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        fs::read(env.uploads().join("home/admin/Документы/отчёт.xlsx")).unwrap(),
        b"XLS"
    );

    // Either side may delete; a planted symlink is never followed.
    let (status, _) = call(&app, &anna, test::TestRequest::delete().uri(&report)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!exchange.join("anna/from-user/отчёт.xlsx").exists());
    let outside = env.dir.join("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, exchange.join("bob")).unwrap();
    let (status, _) = call(&app, &bob, exchange_upload("bob", "x.pdf", b"X")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&app, &admin, exchange_upload("bob", "x.pdf", b"X")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

    // Deleting a user removes their exchange too.
    let (status, _) = call(
        &app,
        &admin,
        test::TestRequest::delete().uri("/admin/users/anna"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!exchange.join("anna").exists());
}
