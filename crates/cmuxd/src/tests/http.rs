use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Serve the router on an ephemeral port. The returned sender must be kept
/// alive: dropping it closes the channel, which fires the graceful-shutdown
/// future and stops the server mid-test.
async fn spawn_server() -> (SocketAddr, broadcast::Sender<()>) {
    let registry = Arc::new(Registry::new());
    let (tx, rx) = broadcast::channel::<()>(1);
    let addr = serve("127.0.0.1:0", registry, rx).await.expect("serve");
    (addr, tx)
}

/// Minimal HTTP/1.1 GET, so the test needs no client dependency.
async fn get(addr: SocketAddr, path: &str) -> String {
    let mut s = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.expect("write");
    let mut out = String::new();
    s.read_to_string(&mut out).await.expect("read");
    out
}

/// The contract after auth was removed: no credential is asked for, and
/// none is required. Access control belongs to whatever fronts the port.
#[tokio::test]
async fn every_route_answers_without_credentials() {
    let (addr, _keepalive) = spawn_server().await;

    let health = get(addr, "/api/health").await;
    assert!(health.starts_with("HTTP/1.1 200"), "{health}");
    assert!(health.contains("\"ok\":true"), "{health}");
    assert!(
        !health.to_lowercase().contains("www-authenticate"),
        "must not challenge for credentials: {health}"
    );

    let list = get(addr, "/api/sessions").await;
    assert!(list.starts_with("HTTP/1.1 200"), "{list}");
    assert!(list.contains("[]"), "empty registry lists nothing: {list}");

    let page = get(addr, "/").await;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("<title>cmuxd</title>"), "{page}");
}

/// The page must not depend on the viewing device having these fonts, so
/// the binary has to actually carry them.
#[test]
fn fonts_are_compiled_into_the_binary() {
    for name in ["mono.woff2", "mono-bold.woff2", "symbols.woff2"] {
        let bytes = embedded_font(name).unwrap_or_else(|| panic!("{name} is not embedded"));
        assert_eq!(&bytes[..4], b"wOF2", "{name} is not a woff2 file");
        assert!(
            bytes.len() > 10_000,
            "{name} is only {} bytes; the vendored file looks wrong",
            bytes.len()
        );
    }
    assert!(embedded_font("../../../etc/passwd").is_none());
    assert!(embedded_font("nope.woff2").is_none());
}

#[tokio::test]
async fn fonts_are_served() {
    let (addr, _keepalive) = spawn_server().await;
    let mut s = tokio::net::TcpStream::connect(addr).await.expect("connect");
    s.write_all(
        b"GET /fonts/symbols.woff2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await
    .expect("write");
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.expect("read");
    let head = String::from_utf8_lossy(&out[..out.len().min(300)]).to_string();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.contains("font/woff2"), "{head}");
    assert!(out.len() > 100_000, "body was only {} bytes", out.len());

    let missing = get(addr, "/fonts/nope.woff2").await;
    assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
}

#[tokio::test]
async fn an_unknown_session_is_a_404() {
    let (addr, _keepalive) = spawn_server().await;
    let res = get(addr, "/api/sessions/42/screen").await;
    assert!(res.starts_with("HTTP/1.1 404"), "{res}");
}
