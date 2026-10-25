//! Two kinds of link to an app a worker runs on this machine.
//!
//! - Inside amux, for the signed-in owner: `/proxy/<port>/` (port_proxy.rs)
//!   and Pages (`/pages/<name>`). Behind amux's sign-in, and on a cloud box
//!   behind its front door.
//! - Public, for anyone with the link: `/pub/<id>/`. The owner (or a worker,
//!   with `amux publish <port>`) creates one for a port; it is listed in the
//!   Pages view and works until revoked. The id is 128 random bits, the only
//!   thing that grants access, and the relay strips amux's cookies and
//!   credentials before anything reaches the app.
//!
//! Both are built from the server's own address ([`public_url`]), so a worker
//! never hands the owner `https://<raw ip>:8824/...` (sandbox, 2026-10-25:
//! mtp-demo did exactly that, because nothing told it the address).
use super::port_proxy;
use super::AppState;
use axum::body::Body;
use axum::extract::{Path as AxPath, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const LINKS_FILE: &str = "public-links.json";
const LEARNED_URL_FILE: &str = "public-url.json";
static WRITE: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Link {
    pub id: String,
    pub port: u16,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub created_at: i64,
}

fn links_path(home: &Path) -> PathBuf {
    home.join(LINKS_FILE)
}

pub(crate) fn load(home: &Path) -> Vec<Link> {
    std::fs::read_to_string(links_path(home))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| serde_json::from_value(v["links"].clone()).ok())
        .unwrap_or_default()
}

fn write_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", ulid::Ulid::new()));
    let r = std::fs::write(&tmp, body).and_then(|_| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path)
    });
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

fn save(home: &Path, links: &[Link]) -> std::io::Result<()> {
    write_atomic(&links_path(home), &serde_json::to_string_pretty(&json!({ "links": links })).unwrap_or_default())
}

/// 128 bits from the OS. No weaker fallback: the id is the whole credential.
fn new_id() -> Option<String> {
    use std::io::Read;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom").ok()?.read_exact(&mut b).ok()?;
    Some(hex::encode(b))
}

pub(crate) fn valid_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Ports a public link may never reach: amux itself (its API and dashboard),
/// the cloud front door, and SSH.
pub(crate) fn refused_port(port: u16) -> Option<&'static str> {
    if port == 0 {
        return Some("not a port");
    }
    if port == crate::config::canonical_port() || (8822..=8824).contains(&port) {
        return Some("that is amux's own port; a public link never exposes amux itself");
    }
    match port {
        8890 => Some("that is the cloud sign-in door"),
        22 => Some("that is SSH"),
        _ => None,
    }
}

/// `https://host[:port]` with no trailing slash, or None.
fn normalize_url(u: &str) -> Option<String> {
    let u = u.trim().trim_end_matches('/');
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://"))?;
    (!rest.is_empty() && !rest.contains('/') && !rest.contains(char::is_whitespace)).then(|| u.to_string())
}

/// The address the owner reaches this amux at. `AMUX_PUBLIC_URL` (server.env;
/// a cloud box sets it to https://<its domain>) wins; otherwise the address the
/// owner last opened the dashboard at. None when neither is known: callers say
/// so rather than guessing.
pub(crate) fn public_url(home: &Path) -> Option<String> {
    if let Some(u) = crate::api::settings::effective_env(home, "AMUX_PUBLIC_URL").as_deref().and_then(normalize_url) {
        return Some(u);
    }
    std::fs::read_to_string(home.join(LEARNED_URL_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["url"].as_str().and_then(normalize_url))
}

fn is_loopback_host(host: &str) -> bool {
    let h = port_proxy::host_only(host).trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost") || h == "0.0.0.0" || h.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The origin a dashboard request was made at, from its Host header. None for
/// loopback (a worker on the box, not the owner's address).
pub(crate) fn origin_of(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(axum::http::header::HOST)?.to_str().ok()?.trim();
    if host.is_empty() || is_loopback_host(host) || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b)) {
        return None;
    }
    let proto = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).filter(|p| *p == "http").unwrap_or("https");
    Some(format!("{proto}://{host}"))
}

/// Remember the address an OWNER opened the dashboard at (the caller checks
/// that it is the owner, so a stranger's Host header cannot redirect workers'
/// links). Written only when it changes.
pub(crate) fn learn_public_url(home: &Path, headers: &HeaderMap) {
    let Some(url) = origin_of(headers) else { return };
    let path = home.join(LEARNED_URL_FILE);
    let current = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    if current.as_ref().and_then(|v| v["url"].as_str()) == Some(url.as_str()) {
        return;
    }
    let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    if write_atomic(&path, &json!({ "url": url, "learned_at": crate::config::now_f64() as i64 }).to_string()).is_ok() {
        tracing::info!(url = %url, measured = true, n_considered = 1, verdict = "public_url_learned",
            "learned the address the owner opens amux at; workers build their links from it");
    }
}

/// What a worker is told about links to what it builds, with this server's
/// real address in it (compose_worker_context).
pub(crate) fn worker_links_guide(home: &Path) -> String {
    let (base, known) = match public_url(home) {
        Some(u) => (u, true),
        None => ("<this amux's address>".to_string(), false),
    };
    let address = if known {
        format!("This amux is reached at {base}. Build every link you give the owner from that address, \
                 never from localhost, 127.0.0.1 or this machine's IP address.")
    } else {
        "This amux's address is not known yet (AMUX_PUBLIC_URL in server.env sets it; otherwise it is \
         learned the first time the owner opens the dashboard). Give the owner the path, such as \
         /proxy/<port>/, and say the address is unknown. Do not guess it from an IP address."
            .to_string()
    };
    format!(
        "{address}\n\n\
         - Inside amux, for the owner (signed in): {base}/proxy/<port>/ opens an app you run on <port>, \
         and a Page is {base}/pages/<name>.\n\
         - Public, for anyone with the link (no amux sign-in): only when the owner wants it shared, \
         run `amux publish <port> [title]`. It prints {base}/pub/<id>/. The owner sees and turns off \
         public links in the Pages view; `amux publish --list` and `amux publish --revoke <id>` do the same."
    )
}

fn link_json(home: &Path, l: &Link) -> Value {
    let base = public_url(home);
    let path = format!("/pub/{}/", l.id);
    json!({
        "id": l.id, "port": l.port, "title": l.title, "created_by": l.created_by, "created_at": l.created_at,
        "path": path,
        "url": base.as_ref().map(|b| format!("{b}{path}")),
        "inside_url": base.as_ref().map(|b| format!("{b}/proxy/{}/", l.port)),
    })
}

fn bad(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

pub(crate) async fn list(State(_): State<AppState>) -> Response {
    let home = crate::config::amux_home();
    let links = load(&home);
    Json(json!({
        "links": links.iter().map(|l| link_json(&home, l)).collect::<Vec<_>>(),
        "public_url": public_url(&home),
        "measured": true, "n_considered": links.len(),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct CreateBody {
    port: Option<u16>,
    #[serde(default)]
    title: String,
}

pub(crate) async fn create(State(_): State<AppState>, headers: HeaderMap, body: Option<Json<CreateBody>>) -> Response {
    let Some(Json(body)) = body else { return bad(StatusCode::BAD_REQUEST, "send {\"port\": <port>}") };
    let Some(port) = body.port else { return bad(StatusCode::BAD_REQUEST, "port is required") };
    if let Some(why) = refused_port(port) {
        tracing::warn!(port, measured = true, n_considered = 1, verdict = "public_link_refused", why, "public link refused");
        return bad(StatusCode::BAD_REQUEST, why);
    }
    let Some(id) = new_id() else {
        return bad(StatusCode::INTERNAL_SERVER_ERROR, "no secure randomness available; no link was made");
    };
    let created_by = headers
        .get("x-amux-session")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("owner")
        .to_string();
    let title: String = body.title.trim().chars().take(120).collect();
    let link = Link { id, port, title, created_by: created_by.clone(), created_at: crate::config::now_f64() as i64 };
    let home = crate::config::amux_home();
    {
        let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
        let mut links = load(&home);
        links.push(link.clone());
        if let Err(e) = save(&home, &links) {
            return bad(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not save the link: {e}"));
        }
    }
    let listening = port_proxy::local_listener(port).await.is_some();
    tracing::warn!(port, created_by = %created_by, listening, measured = true, n_considered = 1,
        verdict = "public_link_created", "public link created: anyone with it can open this port's app");
    let mut out = link_json(&home, &link);
    out["listening"] = json!(listening);
    (StatusCode::CREATED, Json(out)).into_response()
}

pub(crate) async fn revoke(State(_): State<AppState>, AxPath(id): AxPath<String>) -> Response {
    let home = crate::config::amux_home();
    let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut links = load(&home);
    let before = links.len();
    links.retain(|l| l.id != id);
    if links.len() == before {
        return bad(StatusCode::NOT_FOUND, "no such public link");
    }
    if let Err(e) = save(&home, &links) {
        return bad(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not save: {e}"));
    }
    tracing::warn!(measured = true, n_considered = 1, verdict = "public_link_revoked", "public link revoked");
    Json(json!({ "ok": true })).into_response()
}

/// `/pub/<id>/a/b?x=1` -> ("<id>", "/a/b?x=1").
pub(crate) fn parse_pub(uri: &Uri) -> Option<(String, String)> {
    let rest = uri.path().strip_prefix("/pub/")?;
    let (id, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let mut tail = tail.to_string();
    if let Some(q) = uri.query() {
        tail.push('?');
        tail.push_str(q);
    }
    Some((id.to_string(), tail))
}

fn not_found() -> Response {
    port_proxy::page(StatusCode::NOT_FOUND, "Link not found",
        "<p>This link does not exist or was turned off by the person who shared it.</p>")
}

/// Anyone with the link: no amux sign-in. Everything is relayed from loopback,
/// never redirected to the raw port (a cloud box's firewall closes those).
pub(crate) async fn serve(method: Method, headers: HeaderMap, uri: Uri, body: Body) -> Response {
    let Some((id, tail)) = parse_pub(&uri) else { return not_found() };
    if !valid_id(&id) {
        return not_found();
    }
    let home = crate::config::amux_home();
    let Some(link) = load(&home).into_iter().find(|l| l.id == id) else {
        tracing::info!(measured = true, n_considered = 1, verdict = "public_link_unknown", "public link not found");
        return not_found();
    };
    if refused_port(link.port).is_some() {
        return not_found();
    }
    if uri.path() == format!("/pub/{id}") {
        // Without the slash the app's relative URLs would resolve under /pub/.
        return axum::response::Redirect::permanent(&format!("/pub/{id}/{}", uri.query().map(|q| format!("?{q}")).unwrap_or_default())).into_response();
    }
    let Some(local) = port_proxy::local_listener(link.port).await else {
        tracing::warn!(port = link.port, measured = true, n_considered = 1, verdict = "public_link_nothing_listening",
            "public link opened while nothing listens on its port");
        return port_proxy::page(StatusCode::BAD_GATEWAY, "This app is not running",
            "<p>The app behind this link is not running right now. Ask the person who shared it.</p>");
    };
    port_proxy::relay(local, link.port, &format!("/pub/{id}"), &tail, method, &headers, body).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_random_hex_and_only_that_shape_is_accepted() {
        let a = new_id().unwrap();
        let b = new_id().unwrap();
        assert!(valid_id(&a) && valid_id(&b) && a != b);
        for bad in ["", "abc", "../../etc", &a.to_uppercase(), &format!("{a}0")] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn a_public_link_never_reaches_amux_itself() {
        assert!(refused_port(crate::config::canonical_port()).is_some());
        assert!(refused_port(8824).is_some() && refused_port(8823).is_some());
        assert!(refused_port(8890).is_some());
        assert!(refused_port(22).is_some());
        assert!(refused_port(8891).is_none());
    }

    #[test]
    fn the_pub_path_names_its_id_and_keeps_the_page_and_query() {
        let u: Uri = "/pub/abc/x/y.js?v=1".parse().unwrap();
        assert_eq!(parse_pub(&u), Some(("abc".into(), "/x/y.js?v=1".into())));
        let u: Uri = "/pub/abc".parse().unwrap();
        assert_eq!(parse_pub(&u), Some(("abc".into(), "/".into())));
    }

    #[test]
    fn the_learned_address_is_the_owners_and_never_loopback() {
        let mut h = HeaderMap::new();
        h.insert("host", "sandbox.amux.io".parse().unwrap());
        assert_eq!(origin_of(&h).as_deref(), Some("https://sandbox.amux.io"));
        h.insert("host", "desktop.tail5ce8f5.ts.net:8824".parse().unwrap());
        assert_eq!(origin_of(&h).as_deref(), Some("https://desktop.tail5ce8f5.ts.net:8824"));
        for lo in ["localhost:8824", "127.0.0.1:8824", "[::1]:8824", "0.0.0.0"] {
            h.insert("host", lo.parse().unwrap());
            assert_eq!(origin_of(&h), None, "{lo}");
        }
        h.insert("host", "evil.com/x".parse().unwrap());
        assert_eq!(origin_of(&h), None);
    }

    #[test]
    fn the_configured_address_wins_and_is_normalized() {
        assert_eq!(normalize_url("https://sandbox.amux.io/").as_deref(), Some("https://sandbox.amux.io"));
        assert_eq!(normalize_url("sandbox.amux.io"), None);
        assert_eq!(normalize_url("https://a.b/c"), None);
    }

    #[tokio::test]
    async fn an_unknown_or_malformed_id_is_a_404_and_never_a_relay() {
        let r = serve(Method::GET, HeaderMap::new(), "/pub/0123456789abcdef0123456789abcdef/".parse().unwrap(), Body::empty()).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let r = serve(Method::GET, HeaderMap::new(), "/pub/../../api/sessions".parse().unwrap(), Body::empty()).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }
}
