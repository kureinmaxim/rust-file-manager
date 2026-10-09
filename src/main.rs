mod admin;
mod api;
#[cfg(test)]
mod api_tests;
mod auth;
mod categories;
mod config;
mod exchange;
mod files;
#[cfg(test)]
mod files_tests;
mod internal;
mod miniapp;
mod paths;
mod ratelimit;
mod reply;
mod storage;
mod tg_auth;
mod tokens;
#[cfg(test)]
mod upload_routes_tests;
mod uploads;
mod users;

use std::io::Read;

use actix_session::config::PersistentSession;
use actix_session::{storage::CookieSessionStore, SessionMiddleware};
use actix_web::cookie::{time::Duration, SameSite};
use actix_web::middleware::{from_fn, Logger};
use actix_web::{web, App, HttpServer};

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
    std::fs::create_dir_all(config.upload_dir.join(storage::HOME_DIR))?;
    std::fs::create_dir_all(config.upload_dir.join(storage::STAGING_DIR))?;
    let shared_dir = config.upload_dir.join(storage::SHARED_DIR);
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

    let handlebars = web::Data::new(files::templates());

    let session_key = config.session_key();
    let token_keys = web::Data::new(tokens::TokenKeys::derive(&config.secret));
    let link_limiter = web::Data::new(api::link_limiter());
    let upload_locks = web::Data::new(uploads::UploadLocks::default());
    let miniapp_enabled = config.telegram.is_some();
    let config = web::Data::new(config);

    // Hourly removal of chunked uploads abandoned for more than a day.
    let staging_root = config.upload_dir.clone();
    actix_web::rt::spawn(async move {
        let mut interval = actix_web::rt::time::interval(std::time::Duration::from_secs(3600));
        loop {
            interval.tick().await;
            let root = staging_root.clone();
            let removed = actix_web::rt::task::spawn_blocking(move || {
                uploads::cleanup_staging(&root, uploads::STAGING_TTL)
            })
            .await
            .unwrap_or(0);
            if removed > 0 {
                tracing::info!(removed, "removed stale chunked uploads");
            }
        }
    });

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        commit = env!("BUILD_GIT_COMMIT"),
        built = env!("BUILD_DATE"),
        addr = %config.bind_addr,
        upload_dir = %config.upload_dir.display(),
        miniapp = miniapp_enabled,
        "starting server"
    );

    // Exchange arrivals: written by the public server, read by the bot's internal API.
    let exchange_events = web::Data::new(exchange::Events::default());
    let started = web::Data::new(internal::ServerInfo {
        started: std::time::Instant::now(),
    });
    let app_config = config.clone();
    let public_store = user_store.clone();
    let public_events = exchange_events.clone();
    let public_server = HttpServer::new(move || {
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
            .app_data(public_store.clone())
            .app_data(handlebars.clone())
            .app_data(token_keys.clone())
            .app_data(link_limiter.clone())
            .app_data(upload_locks.clone())
            .app_data(public_events.clone())
            // Signed download links carry a token in the path: keep them out of the access log.
            .wrap(Logger::default().exclude_regex("^/d/"))
            .wrap(session_middleware)
            // Mini App API (Bearer tokens) — before the cookie-protected catch-all scope.
            .configure(|cfg| {
                if miniapp_enabled {
                    api::configure(cfg);
                    miniapp::configure(cfg);
                }
            })
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
                    .service(files::move_file)
                    .service(exchange::web_upload)
                    .service(exchange::web_download)
                    .service(exchange::web_delete)
                    .service(exchange::web_save)
                    .service(files::download)
                    .service(files::create_folder)
                    .service(files::rename_folder)
                    .service(files::delete_folder),
            )
    })
    .bind(&config.bind_addr)?
    .run();

    let Some(internal_addr) = config.internal_bind_addr.clone() else {
        return public_server.await;
    };
    // Internal API for the bot: own listener, never proxied by nginx.
    let internal_config = config.clone();
    let internal_server = HttpServer::new(move || {
        App::new()
            .app_data(internal_config.clone())
            .app_data(exchange_events.clone())
            .app_data(user_store.clone())
            .app_data(started.clone())
            .wrap(Logger::default())
            .configure(internal::configure)
    })
    .workers(1)
    .bind(&internal_addr)?
    .run();
    tracing::info!(addr = %internal_addr, "internal API for the bot enabled");
    futures_util::future::try_join(public_server, internal_server).await?;
    Ok(())
}
