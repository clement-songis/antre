#![allow(clippy::unwrap_used, clippy::expect_used)]

use {
    axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    },
    tower::ServiceExt,
};

use {
    super::*,
    crate::{
        events::Visibility,
        service::{CreateDesign, tests::service_with},
    },
};

const HOST: &str = "127.0.0.1:18790";

async fn get(app: &Router, uri: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::HOST, HOST)
                .header(header::COOKIE, "moltis_session=secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 1 << 24)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn csp(response: &Response) -> String {
    response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .to_string()
}

fn assert_common_headers(response: &Response) {
    let headers = response.headers();
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert!(headers.get(header::CONTENT_SECURITY_POLICY).is_some());
}

async fn setup() -> (
    tempfile::TempDir,
    Arc<CanvasService>,
    Router,
    crate::DesignId,
) {
    let (tmp, service, _sink) = service_with(None).await;
    let created = service
        .create(CreateDesign {
            title: "t".into(),
            html: Some(
                "<html><head><link rel=stylesheet href=app.css></head><body>hi</body></html>"
                    .into(),
            ),
            ..CreateDesign::default()
        })
        .await
        .unwrap();
    std::fs::create_dir_all(created.dir.join("assets")).unwrap();
    std::fs::write(created.dir.join("app.css"), "body{}").unwrap();
    std::fs::write(created.dir.join("assets/font.woff2"), b"wOF2").unwrap();
    std::fs::write(created.dir.join(".env"), "SECRET=1").unwrap();
    std::fs::write(tmp.path().join("outside.txt"), "outside").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(tmp.path().join("outside.txt"), created.dir.join("leak.txt"))
        .unwrap();
    let app = router(Arc::clone(&service));
    (tmp, service, app, created.design.id)
}

fn token_path(url: &str) -> String {
    url.trim_start_matches("http://127.0.0.1:18790").to_string()
}

#[tokio::test]
async fn serves_design_with_strict_csp() {
    let (_tmp, service, app, id) = setup().await;
    let link = service.preview_link(&id, None, None).await.unwrap();
    let path = token_path(&link.url);

    let response = get(&app, &path).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_common_headers(&response);
    let policy = csp(&response);
    assert!(policy.starts_with("sandbox allow-scripts;"), "{policy}");
    assert!(policy.contains("connect-src 'none'"));
    assert!(policy.contains("form-action 'none'"));
    assert!(policy.contains(&format!(
        "script-src 'unsafe-inline' http://{HOST}/_vendor/ http://{HOST}{path}"
    )));
    assert!(!policy.contains("allow-same-origin"));
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    assert!(body(response).await.contains("hi"));

    let css = get(&app, &format!("{path}app.css")).await;
    assert_eq!(css.status(), StatusCode::OK);
    assert_eq!(csp(&css), ASSET_POLICY);
    assert_eq!(
        css.headers()[header::CONTENT_TYPE],
        "text/css; charset=utf-8"
    );

    let font = get(&app, &format!("{path}assets/font.woff2")).await;
    assert_eq!(font.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    assert!(
        css.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}

#[tokio::test]
async fn trailing_slash_redirect_keeps_relative_urls_working() {
    let (_tmp, service, app, id) = setup().await;
    let link = service.preview_link(&id, None, None).await.unwrap();
    let path = token_path(&link.url);
    let response = get(&app, path.trim_end_matches('/')).await;
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(response.headers()[header::LOCATION], path.as_str());
}

#[tokio::test]
async fn refuses_traversal_hidden_files_and_symlinks() {
    let (_tmp, service, app, id) = setup().await;
    let link = service.preview_link(&id, None, None).await.unwrap();
    let path = token_path(&link.url);
    for attempt in [
        format!("{path}../../outside.txt"),
        format!("{path}%2e%2e/%2e%2e/outside.txt"),
        format!("{path}..%2f..%2foutside.txt"),
        format!("{path}.env"),
        format!("{path}leak.txt"),
        format!("{path}assets"),
        format!("{path}missing.html"),
    ] {
        let response = get(&app, &attempt).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{attempt}");
        assert_common_headers(&response);
    }
}

#[tokio::test]
async fn refuses_bad_or_foreign_tokens() {
    let (_tmp, service, app, id) = setup().await;
    let link = service.preview_link(&id, None, None).await.unwrap();
    let path = token_path(&link.url);
    let forged = path.replacen(id.as_str(), crate::DesignId::generate().as_str(), 1);
    for attempt in ["/d/garbage/".to_string(), forged, "/d//".to_string()] {
        assert_eq!(
            get(&app, &attempt).await.status(),
            StatusCode::NOT_FOUND,
            "{attempt}"
        );
    }
    assert_eq!(
        get(&app, "/api/canvas/designs").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(get(&app, "/").await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pinned_version_serves_the_snapshot() {
    let (_tmp, service, app, id) = setup().await;
    let dir = service.design_dir(&id);
    std::fs::write(dir.join("index.html"), "<p>live edit</p>").unwrap();
    let v1 = service.preview_link(&id, Some(1), None).await.unwrap();
    let live = service.preview_link(&id, None, None).await.unwrap();
    assert!(
        body(get(&app, &token_path(&v1.url)).await)
            .await
            .contains("hi")
    );
    assert!(
        body(get(&app, &token_path(&live.url)).await)
            .await
            .contains("live edit")
    );
}

#[tokio::test]
async fn rejects_hosts_that_could_inject_csp() {
    let (_tmp, service, app, id) = setup().await;
    let link = service.preview_link(&id, None, None).await.unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(token_path(&link.url))
                .header(header::HOST, "x; script-src *")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn vendor_route_serves_only_known_files() {
    let (_tmp, _service, app, _id) = setup().await;
    let response = get(&app, "/_vendor/preact.js").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    assert_common_headers(&response);
    assert_eq!(
        get(&app, "/_vendor/..%2f..%2fmoltis.db").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/_vendor/unknown.js").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn published_pages_need_the_key_and_stay_framed_by_the_wrapper() {
    let (_tmp, service, app, id) = setup().await;
    let _ = std::fs::remove_file(service.design_dir(&id).join("leak.txt"));
    let published = service
        .publish(&id, None, Visibility::Private, None, None)
        .await
        .unwrap();
    let key = published.access_key.unwrap();
    let share = published.share_id;

    assert_eq!(
        get(&app, &format!("/p/{share}")).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, &format!("/p/{share}?k=wrong")).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, &format!("/p/{share}/content")).await.status(),
        StatusCode::NOT_FOUND
    );

    let wrapper = get(&app, &format!("/p/{share}?k={key}")).await;
    assert_eq!(wrapper.status(), StatusCode::OK);
    assert_common_headers(&wrapper);
    let wrapper_csp = csp(&wrapper);
    assert!(wrapper_csp.contains("frame-ancestors 'none'"));
    assert!(wrapper_csp.contains(&format!("frame-src http://{HOST}")));
    assert!(!wrapper_csp.contains("script-src"));
    let html = body(wrapper).await;
    assert!(html.contains("sandbox=\"allow-scripts\""));
    assert!(html.contains(&format!("/p/{share}/content?k={key}")));
    assert!(html.contains("Do not enter passwords"));

    let content = get(&app, &format!("/p/{share}/content?k={key}")).await;
    assert_eq!(content.status(), StatusCode::OK);
    let content_csp = csp(&content);
    assert!(content_csp.starts_with("sandbox allow-scripts;"));
    assert!(content_csp.contains(&format!("frame-ancestors http://{HOST}")));
    assert!(content_csp.contains("connect-src 'none'"));
    assert!(body(content).await.contains("body{}"), "stylesheet inlined");

    assert_eq!(
        get(&app, "/p/not-a-uuid").await.status(),
        StatusCode::NOT_FOUND
    );
    service.unpublish(&share).await.unwrap();
    assert_eq!(
        get(&app, &format!("/p/{share}?k={key}")).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn published_titles_are_escaped() {
    let (_tmp, service, app, _id) = setup().await;
    let created = service
        .create(CreateDesign {
            title: "<script>alert(1)</script>".into(),
            ..CreateDesign::default()
        })
        .await
        .unwrap();
    let published = service
        .publish(&created.design.id, None, Visibility::Public, None, None)
        .await
        .unwrap();
    let html = body(get(&app, &format!("/p/{}", published.share_id)).await).await;
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!html.contains("<script>alert(1)"));
}

#[tokio::test]
async fn spawn_binds_and_records_address() {
    let (_tmp, service, _app, _id) = setup().await;
    let tmp = tempfile::tempdir().unwrap();
    let fresh = Arc::new(
        CanvasService::new(crate::CanvasServiceParts {
            data_dir: tmp.path().to_path_buf(),
            pool: crate::store::tests::test_pool().await,
            settings: crate::CanvasSettings::default(),
            renderer: None,
            shares: None,
        })
        .unwrap(),
    );
    let (bound, handle) = spawn(Arc::clone(&fresh), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    assert_ne!(bound.port(), 0);
    assert_eq!(fresh.render_port(), Some(bound.port()));
    handle.abort();
    drop(service);
}
