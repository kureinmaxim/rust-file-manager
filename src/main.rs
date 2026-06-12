mod admin;
mod auth;
mod categories;
mod config;
mod files;
mod users;

use std::io::Read;

use actix_session::config::PersistentSession;
use actix_session::{storage::CookieSessionStore, SessionMiddleware};
use actix_web::cookie::{time::Duration, SameSite};
use actix_web::middleware::{from_fn, Logger};
use actix_web::{web, App, HttpServer};
use handlebars::Handlebars;

use crate::config::AppConfig;

const SESSION_TTL_HOURS: i64 = 12;

fn hash_password_and_exit() -> ! {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .expect("failed to read password from stdin");
    let password = input.trim();
    if password.is_empty() {
        eprintln!("Usage: echo 'your-password' | rust-file-manager hash-password");
        std::process::exit(1);
    }
    let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST).expect("bcrypt failed");
    println!("{hash}");
    // Single quotes are required in .env: dotenvy expands $-sequences in
    // unquoted/double-quoted values, which silently corrupts bcrypt hashes.
    eprintln!("\nAdd this line to your .env (single quotes matter):\nADMIN_PASSWORD_HASH='{hash}'");
    std::process::exit(0);
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("hash-password") {
        hash_password_and_exit();
    }

    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,actix_server=warn".into()),
        )
        .init();

    let config = match AppConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Configuration error: {e}");
            std::process::exit(1);
        }
    };

    std::fs::create_dir_all(&config.upload_dir)?;
    std::fs::create_dir_all(config.upload_dir.join(files::HOME_DIR))?;
    let shared_dir = config.upload_dir.join(files::SHARED_DIR);
    std::fs::create_dir_all(&shared_dir)?;
    // Pre-multi-user installs kept categories at the upload root; move them
    // into the shared zone so existing files stay visible.
    for (category, _) in categories::FILE_CATEGORIES {
        let legacy = config.upload_dir.join(category);
        let target = shared_dir.join(category);
        if legacy.is_dir() && !target.exists() {
            std::fs::rename(&legacy, &target)?;
            tracing::info!(category, "migrated legacy category dir into shared zone");
        }
        std::fs::create_dir_all(&target)?;
    }
    for folder in categories::BACKUP_FOLDERS {
        std::fs::create_dir_all(shared_dir.join(categories::BACKUP_PARENT).join(folder))?;
    }

    let user_store = match users::UserStore::load(config.users_file.clone()) {
        Ok(s) => web::Data::new(s),
        Err(e) => {
            eprintln!("Failed to load users file: {e}");
            std::process::exit(1);
        }
    };

    let mut handlebars = Handlebars::new();
    handlebars
        .register_template_string("index", include_str!("../templates/index.html"))
        .expect("invalid index template");
    let handlebars = web::Data::new(handlebars);

    let session_key = config.session_key();
    let config = web::Data::new(config);

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        commit = env!("BUILD_GIT_COMMIT"),
        built = env!("BUILD_DATE"),
        addr = %config.bind_addr,
        upload_dir = %config.upload_dir.display(),
        "starting server"
    );

    let app_config = config.clone();
    HttpServer::new(move || {
        let session_middleware =
            SessionMiddleware::builder(CookieSessionStore::default(), session_key.clone())
                .cookie_secure(app_config.cookie_secure)
                .cookie_same_site(SameSite::Strict)
                .cookie_http_only(true)
                .session_lifecycle(
                    PersistentSession::default().session_ttl(Duration::hours(SESSION_TTL_HOURS)),
                )
                .build();

        App::new()
            .app_data(app_config.clone())
            .app_data(user_store.clone())
            .app_data(handlebars.clone())
            .wrap(Logger::default())
            .wrap(session_middleware)
            .route("/login", web::get().to(auth::login_page))
            .route("/login", web::post().to(auth::login))
            .route("/logout", web::post().to(auth::logout))
            .route("/register", web::get().to(auth::register_page))
            .route("/register", web::post().to(auth::register))
            .service(
                web::scope("/admin")
                    .wrap(from_fn(auth::require_admin))
                    .wrap(from_fn(auth::require_auth))
                    .service(admin::create_invite)
                    .service(admin::delete_user),
            )
            .service(
                web::scope("")
                    .wrap(from_fn(auth::require_auth))
                    .service(files::index)
                    .service(files::upload)
                    .service(files::delete_file)
                    .service(files::rename_file)
                    .service(files::download),
            )
    })
    .bind(&config.bind_addr)?
    .run()
    .await
}
