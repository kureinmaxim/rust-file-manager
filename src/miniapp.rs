//! Telegram Mini App shell: `/tg/` and `/tg/assets/*`, embedded at build time
//! from `miniapp/` (see build.rs), so deployment stays a single binary.

use actix_web::http::header::{self, HeaderValue};
use actix_web::{web, HttpRequest, HttpResponse};

pub struct Asset {
    pub path: &'static str,
    pub body: &'static [u8],
    pub content_type: &'static str,
    pub etag: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/miniapp_assets.rs"));

/// The only external script is Telegram's own SDK. No inline scripts or
/// styles; framing is allowed only for Telegram Web, which embeds Mini Apps
/// in an iframe.
const CSP: &str = "default-src 'none'; script-src 'self' https://telegram.org; \
    style-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; \
    connect-src 'self'; font-src 'self'; base-uri 'none'; form-action 'none'; \
    frame-ancestors https://web.telegram.org";

/// GET and HEAD: a HEAD that matched no route here would fall through to the
/// web UI's login guard and get its 401.
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::resource("/tg")
            .route(web::get().to(redirect))
            .route(web::head().to(redirect)),
    )
    .service(
        web::resource("/tg/")
            .route(web::get().to(index))
            .route(web::head().to(index)),
    )
    .service(
        web::resource("/tg/assets/{path:.*}")
            .route(web::get().to(asset))
            .route(web::head().to(asset)),
    );
}

async fn redirect() -> HttpResponse {
    HttpResponse::MovedPermanently()
        .insert_header((header::LOCATION, "/tg/"))
        .finish()
}

fn find(path: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|a| a.path == path)
}

fn serve(req: &HttpRequest, asset: &Asset) -> HttpResponse {
    let not_modified = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|tag| tag.trim() == asset.etag));
    let mut response = if not_modified {
        HttpResponse::NotModified().finish()
    } else {
        HttpResponse::Ok()
            .content_type(asset.content_type)
            .body(asset.body)
    };
    let headers = response.headers_mut();
    headers.insert(header::ETAG, HeaderValue::from_static(asset.etag));
    // Revalidate every time: cheap 304s, and a new binary is picked up at once.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    response
}

async fn index(req: HttpRequest) -> HttpResponse {
    match find("index.html") {
        Some(asset) => serve(&req, asset),
        None => HttpResponse::NotFound().finish(),
    }
}

async fn asset(req: HttpRequest, path: web::Path<String>) -> HttpResponse {
    match find(&format!("assets/{}", path.into_inner())) {
        Some(asset) => serve(&req, asset),
        None => HttpResponse::NotFound().finish(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::http::StatusCode;
    use actix_web::{test, App};

    #[actix_web::test]
    async fn serves_embedded_assets_with_csp_and_etag() {
        let app = test::init_service(App::new().configure(configure)).await;
        let resp =
            test::call_service(&app, test::TestRequest::get().uri("/tg/").to_request()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let csp = resp
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(csp.contains("frame-ancestors https://web.telegram.org"));
        assert!(!csp.contains("unsafe-inline"));
        let etag = resp.headers().get(header::ETAG).unwrap().clone();

        let cached = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/tg/")
                .insert_header((header::IF_NONE_MATCH, etag))
                .to_request(),
        )
        .await;
        assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);

        let head = test::call_service(
            &app,
            test::TestRequest::default()
                .method(actix_web::http::Method::HEAD)
                .uri("/tg/")
                .to_request(),
        )
        .await;
        assert_eq!(head.status(), StatusCode::OK);
        assert!(head.headers().contains_key(header::CONTENT_SECURITY_POLICY));

        let js = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/tg/assets/app.js")
                .to_request(),
        )
        .await;
        assert_eq!(js.status(), StatusCode::OK);
        assert!(js
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/javascript"));

        for missing in [
            "/tg/assets/../Cargo.toml",
            "/tg/assets/nope.js",
            "/tg/assets/VENDOR.md",
        ] {
            let resp =
                test::call_service(&app, test::TestRequest::get().uri(missing).to_request()).await;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{missing}");
        }
    }
}
