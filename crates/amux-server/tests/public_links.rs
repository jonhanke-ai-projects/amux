//! Public links (`/pub/<id>/`) through the composed router, with an owner
//! token set: the list and its changes need the owner, the link itself needs
//! nothing but its id, and nothing of amux's identity reaches the app.

use amux_server::api::{router, AppState};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;

const TOKEN: &str = "owner-secret-for-test";

fn app() -> axum::Router {
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let home = HOME.get_or_init(|| tempfile::tempdir().unwrap());
    // Links live under the amux home; this binary owns its own.
    unsafe { std::env::set_var("AMUX_HOME", home.path()) };
    let store = amux_server::db::Store::open(&home.path().join("t.db")).unwrap();
    router(AppState {
        store: std::sync::Arc::new(store),
        started: std::time::Instant::now(),
        build_hash: "test".into(),
        auth_token: Some(TOKEN.into()),
        reconciled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
    })
}

async fn call(method: &str, path: &str, owner: bool, body: Option<&str>, cookie: Option<&str>) -> (StatusCode, String, String) {
    let mut b = Request::builder().method(method).uri(path).header(header::HOST, "sandbox.amux.io");
    if owner {
        b = b.header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
    }
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    let req = match body {
        Some(j) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(j.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let res = app().oneshot(req).await.unwrap();
    let status = res.status();
    let loc = res.headers().get(header::LOCATION).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    (status, loc, String::from_utf8_lossy(&body).to_string())
}

/// A loopback-only app that answers every request with the request head it got.
async fn echo_app() -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                let head = String::from_utf8_lossy(&buf[..n]).replace('<', "&lt;");
                let body = format!("<h1>public app</h1><pre>{head}</pre>");
                let _ = s
                    .write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes())
                    .await;
            });
        }
    });
    port
}

#[tokio::test]
async fn a_public_link_opens_without_sign_in_and_only_the_owner_manages_links() {
    let port = echo_app().await;

    // Managing links is the owner's.
    let (st, _, _) = call("GET", "/api/public-links", false, None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "the list must need the owner");
    let (st, _, _) = call("POST", "/api/public-links", false, Some(&format!(r#"{{"port":{port}}}"#)), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "creating must need the owner");

    // amux itself is never published.
    let (st, _, body) = call("POST", "/api/public-links", true, Some(r#"{"port":8824}"#), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");

    let (st, _, body) = call("POST", "/api/public-links", true, Some(&format!(r#"{{"port":{port},"title":"demo"}}"#)), None).await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    let link: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = link["id"].as_str().unwrap().to_string();
    assert_eq!(link["path"], format!("/pub/{id}/"));
    assert_eq!(link["listening"], true);

    // Anyone with the link: no token, no cookie of amux's reaches the app.
    let (st, _, body) = call("GET", &format!("/pub/{id}/page?x=1"), false, None,
        Some("__Host-amux_owner=o; amux_door=d; amux_member=m; appsid=1")).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.contains("<h1>public app</h1>"), "{body}");
    assert!(body.contains("GET /page?x=1 "), "the app sees its own path: {body}");
    assert!(body.contains("appsid=1"), "the app's own cookie passes: {body}");
    for leaked in ["amux_owner", "amux_door", "amux_member", TOKEN] {
        assert!(!body.contains(leaked), "{leaked} reached the app: {body}");
    }
    let (st, _, body) = call("GET", &format!("/pub/{id}/"), false, None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("GET / "), "the link's root is the app's root, never the dashboard: {body}");
    let (st, loc, _) = call("GET", &format!("/pub/{id}"), false, None, None).await;
    assert!(st.is_redirection() && loc == format!("/pub/{id}/"), "{st} {loc}");

    // An id nobody created, or a path trick, is a 404 and never the API.
    let (st, _, _) = call("GET", "/pub/0123456789abcdef0123456789abcdef/", false, None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, body) = call("GET", "/pub/../api/public-links", false, None, None).await;
    assert!(!body.contains(&id), "a path trick listed the links: {st} {body}");

    // The owner sees it, and turning it off ends it.
    let (st, _, body) = call("GET", "/api/public-links", true, None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains(&id), "{body}");
    let (st, _, _) = call("DELETE", &format!("/api/public-links/{id}"), false, None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "revoking must need the owner");
    let (st, _, _) = call("DELETE", &format!("/api/public-links/{id}"), true, None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = call("GET", &format!("/pub/{id}/"), false, None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "a revoked link still opened");
}
