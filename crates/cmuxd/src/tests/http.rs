use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Serve the router on an ephemeral port. The returned sender must be kept
/// alive: dropping it closes the channel, which fires the graceful-shutdown
/// future and stops the server mid-test.
async fn spawn_server() -> (SocketAddr, broadcast::Sender<()>) {
    let (addr, tx, _registry) = spawn_server_with_registry().await;
    (addr, tx)
}

/// Same, but hands back the registry for tests that need to reach a
/// session directly rather than over HTTP.
async fn spawn_server_with_registry() -> (SocketAddr, broadcast::Sender<()>, Arc<Registry>) {
    let registry = Arc::new(Registry::new());
    let (tx, rx) = broadcast::channel::<()>(1);
    let addr = serve("127.0.0.1:0", registry.clone(), rx)
        .await
        .expect("serve");
    (addr, tx, registry)
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

/// One request with a body, so the tests need no HTTP client dependency.
async fn send(addr: SocketAddr, method: &str, path: &str, ctype: &str, body: &str) -> String {
    let mut s = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
         Content-Length: {}\r\n",
        body.len()
    );
    if !ctype.is_empty() {
        req.push_str(&format!("Content-Type: {ctype}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).await.expect("write");
    let mut out = String::new();
    s.read_to_string(&mut out).await.expect("read");
    out
}

async fn post_json(addr: SocketAddr, path: &str, body: &str) -> String {
    send(addr, "POST", path, "application/json", body).await
}

fn status(res: &str) -> &str {
    res.lines().next().unwrap_or("")
}

/// Poll a route until its body holds `needle`, or give up. Output arrives
/// through a pty read thread, so there is nothing to await on directly.
async fn wait_for(addr: SocketAddr, path: &str, needle: &str) -> String {
    for _ in 0..60 {
        let res = get(addr, path).await;
        if res.contains(needle) {
            return res;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    get(addr, path).await
}

async fn spawn_over_http(addr: SocketAddr, label: &str, argv: &[&str]) -> String {
    let cmd = argv
        .iter()
        .map(|a| format!("{:?}", a))
        .collect::<Vec<_>>()
        .join(",");
    let res = post_json(
        addr,
        "/api/sessions",
        &format!(r#"{{"cmd":[{cmd}],"cwd":"/tmp","label":"{label}"}}"#),
    )
    .await;
    assert!(
        res.starts_with("HTTP/1.1 201"),
        "spawning {label} failed: {res}"
    );
    res
}

#[tokio::test]
async fn a_session_spawned_over_http_is_listed() {
    let (addr, _keepalive) = spawn_server().await;
    spawn_over_http(addr, "web", &["/bin/sleep", "30"]).await;

    let list = get(addr, "/api/sessions").await;
    assert!(list.contains("\"label\":\"web\""), "{list}");
    assert!(list.contains("\"alive\":true"), "{list}");

    let one = get(addr, "/api/sessions/1").await;
    assert!(one.starts_with("HTTP/1.1 200"), "{one}");
    assert!(one.contains("\"cwd\":\"/tmp\""), "{one}");
}

#[tokio::test]
async fn spawning_an_unknown_program_is_a_bad_request_naming_it() {
    let (addr, _keepalive) = spawn_server().await;
    let res = post_json(
        addr,
        "/api/sessions",
        r#"{"cmd":["definitely-not-a-real-binary"],"cwd":"/tmp"}"#,
    )
    .await;
    assert_eq!(status(&res), "HTTP/1.1 400 Bad Request", "{res}");
    assert!(res.contains("definitely-not-a-real-binary"), "{res}");
    assert!(get(addr, "/api/sessions").await.contains("[]"), "it stuck");
}

#[tokio::test]
async fn a_spawn_body_without_a_size_gets_the_default_grid() {
    let (addr, _keepalive) = spawn_server().await;
    let res = post_json(
        addr,
        "/api/sessions",
        r#"{"cmd":["/bin/sleep","30"],"cwd":"/tmp"}"#,
    )
    .await;
    assert!(res.contains("\"rows\":24"), "{res}");
    assert!(res.contains("\"cols\":80"), "{res}");
}

#[tokio::test]
async fn a_malformed_spawn_body_is_refused() {
    let (addr, _keepalive) = spawn_server().await;
    for body in [r#"{"cmd":"not a list"}"#, "{not json at all", "{}"] {
        let res = post_json(addr, "/api/sessions", body).await;
        assert!(
            status(&res).starts_with("HTTP/1.1 4"),
            "body {body:?} should be refused, got: {}",
            status(&res)
        );
    }
}

#[tokio::test]
async fn deleting_a_session_removes_it_and_says_so_only_once() {
    let (addr, _keepalive) = spawn_server().await;
    spawn_over_http(addr, "doomed", &["/bin/sleep", "30"]).await;

    let first = send(addr, "DELETE", "/api/sessions/1", "", "").await;
    assert_eq!(status(&first), "HTTP/1.1 204 No Content", "{first}");
    assert!(get(addr, "/api/sessions").await.contains("[]"));

    let second = send(addr, "DELETE", "/api/sessions/1", "", "").await;
    assert_eq!(status(&second), "HTTP/1.1 404 Not Found", "{second}");
}

#[tokio::test]
async fn the_screen_route_shows_what_the_session_printed() {
    let (addr, _keepalive) = spawn_server().await;
    spawn_over_http(
        addr,
        "printer",
        &["/bin/sh", "-c", "echo SCREEN-MARKER; sleep 30"],
    )
    .await;

    let screen = wait_for(addr, "/api/sessions/1/screen", "SCREEN-MARKER").await;
    assert!(screen.contains("text/plain"), "{screen}");
    assert!(screen.contains("SCREEN-MARKER"), "{screen}");

    let buffer = wait_for(addr, "/api/sessions/1/buffer", "SCREEN-MARKER").await;
    assert!(buffer.contains("application/octet-stream"), "{buffer}");
    assert!(buffer.contains("SCREEN-MARKER"), "{buffer}");
}

#[tokio::test]
async fn posted_input_reaches_the_pty() {
    let (addr, _keepalive) = spawn_server().await;
    spawn_over_http(
        addr,
        "shell",
        &["/bin/sh", "-c", "read line; echo GOT-$line"],
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let res = send(addr, "POST", "/api/sessions/1/input", "", "typed\n").await;
    assert_eq!(status(&res), "HTTP/1.1 204 No Content", "{res}");

    let screen = wait_for(addr, "/api/sessions/1/screen", "GOT-typed").await;
    assert!(
        screen.contains("GOT-typed"),
        "input never arrived: {screen}"
    );
}

#[tokio::test]
async fn resizing_works_while_nothing_is_attached() {
    let (addr, _keepalive) = spawn_server().await;
    spawn_over_http(addr, "sizer", &["/bin/sleep", "30"]).await;

    let res = post_json(addr, "/api/sessions/1/resize", r#"{"rows":30,"cols":100}"#).await;
    assert_eq!(status(&res), "HTTP/1.1 204 No Content", "{res}");

    let info = get(addr, "/api/sessions/1").await;
    assert!(info.contains("\"rows\":30"), "{info}");
    assert!(info.contains("\"cols\":100"), "{info}");
}

/// The other half: with a client attached, the pty runs at the minimum of
/// the attached sizes, so a one-shot resize has no attachment to speak for.
#[tokio::test]
async fn resizing_is_refused_while_a_client_is_attached() {
    let (addr, _keepalive, registry) = spawn_server_with_registry().await;
    spawn_over_http(addr, "shared", &["/bin/sleep", "30"]).await;
    let sess = registry.get(1).await.expect("the session");
    sess.set_client_size(registry.alloc_client_id(), 20, 60)
        .expect("attach");

    let res = post_json(addr, "/api/sessions/1/resize", r#"{"rows":30,"cols":100}"#).await;
    assert_eq!(status(&res), "HTTP/1.1 409 Conflict", "{res}");
    assert!(res.contains("client(s) attached"), "{res}");

    let info = get(addr, "/api/sessions/1").await;
    assert!(
        info.contains("\"rows\":20"),
        "the refused resize still took effect: {info}"
    );
}

#[tokio::test]
async fn health_reports_the_protocol_version_and_the_session_count() {
    let (addr, _keepalive) = spawn_server().await;
    let empty = get(addr, "/api/health").await;
    assert!(
        empty.contains(&format!("\"protocol\":{}", cmux_proto::PROTOCOL_VERSION)),
        "{empty}"
    );
    assert!(empty.contains("\"sessions\":0"), "{empty}");

    spawn_over_http(addr, "one", &["/bin/sleep", "30"]).await;
    let one = get(addr, "/api/health").await;
    assert!(one.contains("\"sessions\":1"), "{one}");
}

/// The daemon reaps the child and reports how it went, so a browser can
/// tell a finished session from an idle one.
#[tokio::test]
async fn a_finished_session_is_reported_as_exited() {
    let (addr, _keepalive) = spawn_server().await;
    spawn_over_http(addr, "shortlived", &["/bin/sh", "-c", "exit 3"]).await;

    let info = wait_for(addr, "/api/sessions/1", "\"alive\":false").await;
    assert!(info.contains("\"alive\":false"), "{info}");
    assert!(info.contains("\"exit_status\":\"exited 3\""), "{info}");
    assert!(
        get(addr, "/api/sessions").await.contains("shortlived"),
        "a finished session should stay listed so it can be seen"
    );
}

#[tokio::test]
async fn writing_routes_need_no_credentials_either() {
    let (addr, _keepalive) = spawn_server().await;
    let res = post_json(
        addr,
        "/api/sessions",
        r#"{"cmd":["/bin/sleep","30"],"cwd":"/tmp"}"#,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    assert!(
        !res.to_lowercase().contains("www-authenticate"),
        "must not challenge for credentials: {res}"
    );
}
