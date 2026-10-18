//! `/proxy/<port>/<path>`: open a dev server running on this machine from
//! any device that can reach amux.
//!
//! Workers hand the owner links of this shape for a local dev server
//! (`https://desktop.<tailnet>.ts.net:8824/proxy/3077/?hero=texture`). Until
//! 2026-10-17 no such route existed: the SPA catch-all answered with the
//! dashboard, so the owner on a phone landed on amux instead of the page.
//!
//! The route sends the browser to the SAME host on the dev server's own port
//! rather than relaying bytes. Dev servers write root-relative URLs
//! (`/@vite/client`, `/fonts/x.woff2`, `/_next/...`) and open HMR websockets
//! on their own origin; under a `/proxy/3077/` prefix every one of those
//! resolves against amux and breaks. On its own port the page is exactly what
//! the worker saw locally.
//!
//! So the route first proves the port answers at the address the browser
//! used. A server bound to 127.0.0.1 only cannot be reached that way, and the
//! page says so with the fix, instead of a redirect into a dead connection.
use super::{has_owner_session, peer_ip, tailnet_auth, AppState, Peer};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Redirect, Response};
use std::net::IpAddr;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);

/// `/proxy/3077/a/b?x=1` -> (3077, "/a/b?x=1"). None when the port is not a
/// usable TCP port number.
pub(crate) fn parse_target(uri: &Uri) -> Option<(u16, String)> {
    let rest = uri.path().strip_prefix("/proxy/")?;
    let (port, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let port: u16 = port.parse().ok().filter(|p| *p > 0)?;
    let mut tail = tail.to_string();
    if let Some(q) = uri.query() {
        tail.push('?');
        tail.push_str(q);
    }
    Some((port, tail))
}

/// The host part of a Host header, without its port. Keeps IPv6 brackets so
/// the result can be put straight back into a URL.
pub(crate) fn host_only(authority: &str) -> &str {
    if authority.starts_with('[') {
        return authority
            .find(']')
            .map_or(authority, |i| &authority[..=i]);
    }
    authority.split(':').next().unwrap_or(authority)
}

fn page(status: StatusCode, title: &str, body: &str) -> Response {
    let html = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\">\
         <meta name=\"color-scheme\" content=\"dark light\"><title>{t}</title>\
         <style>body{{font-family:system-ui,sans-serif;max-width:36em;margin:18vh auto 0;padding:0 16px;line-height:1.5}}code{{word-break:break-all}}</style>\
         </head><body><h1 style=\"font-size:1.3em\">{t}</h1>{body}<p><a href=\"/\">Back to amux</a></p></body></html>",
        t = html_escape(title),
    );
    (status, Html(html)).into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn answers(host: &str, port: u16) -> bool {
    let name = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let addrs = match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((name, port))).await {
        Ok(Ok(a)) => a.collect::<Vec<_>>(),
        _ => return false,
    };
    for addr in addrs {
        if let Ok(Ok(_)) = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr)).await {
            return true;
        }
    }
    false
}

pub(crate) async fn proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    peer: Peer,
) -> Response {
    let peer = peer_ip(peer);
    let owner = state.auth_token.is_none()
        || has_owner_session(&state, &headers)
        || peer.is_some_and(|ip| ip.is_loopback())
        || match peer {
            Some(ip) => tailnet_auth::verified(ip).await,
            None => false,
        };
    if !owner {
        tracing::info!(path = %uri.path(), measured = true, n_considered = 1,
            verdict = "port_proxy_not_signed_in", "proxy link opened without an owner session; sent to sign in");
        return Redirect::to("/").into_response();
    }
    let Some((port, tail)) = parse_target(&uri) else {
        return page(StatusCode::BAD_REQUEST, "Not a port",
            "<p>A proxy link looks like <code>/proxy/3077/</code>: a port number from 1 to 65535, then the page.</p>");
    };
    let authority = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| uri.authority().map(|a| a.to_string()))
        .unwrap_or_else(|| "localhost".to_string());
    let host = host_only(&authority).to_string();
    let own_port = authority
        .rsplit_once(':')
        .filter(|(h, _)| !h.is_empty() && !authority.ends_with(']'))
        .and_then(|(_, p)| p.parse::<u16>().ok());
    if own_port == Some(port) {
        return page(StatusCode::BAD_REQUEST, "That is amux's own port",
            &format!("<p>Port {port} is the amux server itself. Use the port the dev server printed.</p>"));
    }
    let local = answers("127.0.0.1", port).await || answers("::1", port).await;
    if !local {
        tracing::warn!(port, host = %host, measured = true, n_considered = 1,
            verdict = "port_proxy_nothing_listening", "proxy link to a port nothing on this machine listens on");
        return page(StatusCode::BAD_GATEWAY, "Nothing is running on that port",
            &format!("<p>No server is listening on port {port} on this machine. It may have stopped; ask the worker that sent the link to start it again.</p>"));
    }
    if !answers(&host, port).await {
        tracing::warn!(port, host = %host, measured = true, n_considered = 1,
            verdict = "port_proxy_loopback_only", "dev server listens on loopback only; unreachable at the host the browser used");
        return page(StatusCode::BAD_GATEWAY, "This server only accepts connections from its own machine",
            &format!("<p>Port {port} is running, but it listens on 127.0.0.1 only, so <code>{h}</code> cannot reach it.</p>\
                      <p>Restart it listening on every interface, for example <code>--host 0.0.0.0</code> for Vite or Next, \
                      or <code>HOST=0.0.0.0</code>, and open this link again.</p>", h = html_escape(&host)));
    }
    let target = format!("http://{host}:{port}{tail}");
    let from_loopback = peer.is_some_and(|ip: IpAddr| ip.is_loopback());
    tracing::info!(port, host = %host, from_loopback, measured = true, n_considered = 1,
        verdict = "port_proxy_redirect", "proxy link sent to the dev server's own port");
    Redirect::temporary(&target).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_path_names_its_port_and_keeps_the_page_and_query() {
        let u: Uri = "/proxy/3077/?hero=texture".parse().unwrap();
        assert_eq!(parse_target(&u), Some((3077, "/?hero=texture".to_string())));
        let u: Uri = "/proxy/5173".parse().unwrap();
        assert_eq!(parse_target(&u), Some((5173, "/".to_string())));
        let u: Uri = "/proxy/8080/a/b.js".parse().unwrap();
        assert_eq!(parse_target(&u), Some((8080, "/a/b.js".to_string())));
        for bad in ["/proxy/0/", "/proxy/70000/", "/proxy/abc/", "/proxy/"] {
            assert_eq!(parse_target(&bad.parse().unwrap()), None, "{bad}");
        }
    }

    #[test]
    fn the_host_keeps_its_name_and_drops_its_port() {
        assert_eq!(host_only("desktop.tail5ce8f5.ts.net:8824"), "desktop.tail5ce8f5.ts.net");
        assert_eq!(host_only("localhost"), "localhost");
        assert_eq!(host_only("[fd7a:115c:a1e0::1]:8824"), "[fd7a:115c:a1e0::1]");
    }

    #[tokio::test]
    async fn a_listening_port_answers_and_a_closed_one_does_not() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(answers("127.0.0.1", port).await);
        drop(l);
        assert!(!answers("127.0.0.1", port).await);
    }
}
