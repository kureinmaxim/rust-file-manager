//! Regression for collection URLs cached with a trailing slash by old nginx.
//! Uses the production API and authorization on an isolated synthetic store.
use std::fs;
use std::path::PathBuf;

use actix_web::http::{header, StatusCode};
use actix_web::{test, web, App};
use serde_json::{json, Value};

use crate::api;
use crate::config::AppConfig;
use crate::tokens::{issue_access, AccessClaims, TokenKeys};
use crate::uploads::UploadLocks;
use crate::users::UserStore;

struct TestDirectory(PathBuf);

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[actix_web::test]
async fn both_collection_urls_upload_a_photo_with_the_same_authorization() {
    for collection in ["/api/v1/uploads", "/api/v1/uploads/"] {
        let dir = TestDirectory(crate::storage::test_directory("upload-routes"));
        let config = web::Data::new(AppConfig::for_tests(dir.0.join("uploads")));
        let store = web::Data::new(UserStore::load(dir.0.join("users.json")).unwrap());
        store.add_user("anna", "synthetic-anna-password").unwrap();
        store.add_user("bob", "synthetic-bob-password").unwrap();
        store.link_telegram("anna", "admin", 101).unwrap();
        store.link_telegram("bob", "admin", 202).unwrap();
        let keys = web::Data::new(TokenKeys::derive(&config.secret));
        let token_for = |username: &str, tg| {
            let account = store.account(username, "admin").unwrap();
            let now = crate::reply::now_secs();
            issue_access(
                &keys,
                &AccessClaims {
                    u: username.into(),
                    tv: account.token_version,
                    tg,
                    iat: now,
                    exp: now + 3600,
                },
            )
        };
        let anna = token_for("anna", 101);
        let bob = token_for("bob", 202);
        let app = test::init_service(
            App::new()
                .app_data(config.clone())
                .app_data(store.clone())
                .app_data(keys)
                .app_data(web::Data::new(UploadLocks::default()))
                .app_data(web::Data::new(crate::exchange::Events::default()))
                .configure(api::configure),
        )
        .await;
        let auth = |req: test::TestRequest, token: &str| {
            req.insert_header((header::AUTHORIZATION, format!("Bearer {token}")))
        };
        for url in ["/api/v1/uploads", "/api/v1/uploads/"] {
            for req in [
                test::TestRequest::get().uri(url),
                test::TestRequest::post().uri(url).set_json(json!({})),
            ] {
                let response = test::call_service(&app, req.to_request()).await;
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            }
            let response = test::call_service(
                &app,
                auth(test::TestRequest::get().uri(url), "synthetic-invalid-token").to_request(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let response = test::call_service(
                &app,
                auth(test::TestRequest::get().uri(url), &anna).to_request(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
        }
        let photo = b"synthetic-photo".repeat(165000);
        let response = test::call_service(
            &app,
            auth(test::TestRequest::post().uri(collection), &anna)
                .set_json(json!({ "scope": "my", "name": "image.jpg", "size": photo.len() }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let created: Value = test::read_body_json(response).await;
        let id = created["id"].as_str().unwrap();
        let item_url = format!("/api/v1/uploads/{id}");
        for url in ["/api/v1/uploads", "/api/v1/uploads/"] {
            let listed: Value = test::call_and_read_body_json(
                &app,
                auth(test::TestRequest::get().uri(url), &anna).to_request(),
            )
            .await;
            assert_eq!(listed["uploads"].as_array().unwrap().len(), 1);
        }
        let response = test::call_service(
            &app,
            auth(test::TestRequest::get().uri(&item_url), &bob).to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let mut offset = 0;
        for chunk in photo.chunks(1024 * 1024) {
            let response = test::call_service(
                &app,
                auth(test::TestRequest::put().uri(&item_url), &anna)
                    .insert_header(("Upload-Offset", offset.to_string()))
                    .set_payload(chunk.to_vec())
                    .to_request(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let received: Value = test::read_body_json(response).await;
            offset += chunk.len();
            assert_eq!(received["offset"], offset);
        }
        let response = test::call_service(
            &app,
            auth(
                test::TestRequest::post().uri(&format!("{item_url}/complete")),
                &anna,
            )
            .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            fs::read(config.upload_dir.join("home/anna/Фото/image.jpg")).unwrap(),
            photo
        );
        store.bump_token_version("anna", "admin").unwrap();
        let response = test::call_service(
            &app,
            auth(test::TestRequest::get().uri(collection), &anna).to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
