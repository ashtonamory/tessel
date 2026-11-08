//! A small HTTP/1.1 server for the preview (§13.2), on its own thread and
//! tokio runtime. It listens on `127.0.0.1` only, on a port the system
//! picks, and answers only under a random 128-bit path segment and only to
//! `Host: 127.0.0.1:<port>` or `localhost:<port>`, so another local user
//! or a web page (through DNS rebinding) cannot read the design.
//!
//! Routes under `/<token>/`:
//! - `` (the shell page), `state` (JSON about the published page),
//! - `events` (server-sent events: the generation after each publish),
//! - `page/<path>` (the export's files).
//!
//! Files are served from memory; nothing is written to disk.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch};

use crate::Page;

const SHELL: &str = include_str!("shell.html");
/// Largest request head accepted.
const MAX_HEAD: usize = 16 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// A comment line keeps idle event streams from timing out.
const HEARTBEAT: Duration = Duration::from_secs(15);

struct Shared {
    page: Mutex<Option<Arc<Page>>>,
    /// Bumped on every publish; event streams send it.
    generation: watch::Sender<u64>,
    /// Shells connected to `events`.
    clients: AtomicUsize,
    token: String,
    port: u16,
}

/// The running preview server. Dropping it stops the server.
pub struct Server {
    shared: Arc<Shared>,
    url: String,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    /// Bind `127.0.0.1` on a free port and start serving.
    pub fn start() -> std::io::Result<Server> {
        let std_listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        std_listener.set_nonblocking(true)?;
        let port = std_listener.local_addr()?.port();
        let token = uuid::Uuid::new_v4().simple().to_string();
        let (generation, _) = watch::channel(0);
        let shared = Arc::new(Shared {
            page: Mutex::new(None),
            generation,
            clients: AtomicUsize::new(0),
            token: token.clone(),
            port,
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()?;
        let (stop, stopped) = oneshot::channel::<()>();
        let serving = shared.clone();
        let thread = std::thread::Builder::new()
            .name("tessel-preview".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let listener = match TcpListener::from_std(std_listener) {
                        Ok(l) => l,
                        Err(e) => {
                            log::error!("preview server: {e}");
                            return;
                        }
                    };
                    tokio::select! {
                        _ = accept(listener, serving) => {}
                        _ = stopped => {}
                    }
                });
                // Dropping the runtime cancels open connections.
            })?;
        Ok(Server {
            url: format!("http://127.0.0.1:{port}/{token}/"),
            shared,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    /// The shell page's address.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Show `page` from now on, and tell connected shells to reload.
    pub fn publish(&self, page: Page) {
        *self.shared.page.lock().expect("preview page lock") = Some(Arc::new(page));
        self.shared.generation.send_modify(|g| *g += 1);
    }

    /// How many shells are connected, i.e. preview windows open.
    pub fn clients(&self) -> usize {
        self.shared.clients.load(Ordering::SeqCst)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

async fn accept(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(serve(stream, shared.clone()));
            }
            Err(e) => {
                log::warn!("preview server: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// A parsed request head: the method, the path without its query, and
/// the `Host` header.
#[derive(Debug, PartialEq)]
struct Head {
    method: String,
    path: String,
    host: Option<String>,
}

fn parse_head(head: &str) -> Option<Head> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?;
    let path = target.split(['?', '#']).next().unwrap_or("").to_string();
    let host = lines.find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("host")
            .then(|| v.trim().to_string())
    });
    Some(Head { method, path, host })
}

async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            return String::from_utf8(buf[..end].to_vec()).ok();
        }
        if buf.len() > MAX_HEAD {
            return None;
        }
    }
}

/// What a request asks for, once it passed the checks.
#[derive(Debug, PartialEq)]
enum Route<'a> {
    Shell,
    State,
    Events,
    File(&'a str),
}

/// Check the host and token and find the route; `Err` is the status line.
fn route<'a>(head: &'a Head, token: &str, port: u16) -> Result<Route<'a>, &'static str> {
    let host_ok = head
        .host
        .as_deref()
        .is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}"));
    if !host_ok {
        return Err("403 Forbidden");
    }
    if head.method != "GET" && head.method != "HEAD" {
        return Err("405 Method Not Allowed");
    }
    let rest = head
        .path
        .strip_prefix('/')
        .and_then(|p| p.strip_prefix(token))
        .and_then(|p| p.strip_prefix('/'))
        .ok_or("404 Not Found")?;
    Ok(match rest {
        "" => Route::Shell,
        "state" => Route::State,
        "events" => Route::Events,
        _ => Route::File(rest.strip_prefix("page/").ok_or("404 Not Found")?),
    })
}

fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    match ext.to_ascii_lowercase().as_str() {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

async fn respond(
    stream: &mut TcpStream,
    status: &str,
    ctype: &str,
    body: &[u8],
    head_only: bool,
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    if !head_only {
        stream.write_all(body).await?;
    }
    stream.flush().await
}

fn state_json(page: Option<&Page>, generation: u64) -> String {
    let v = match page {
        Some(p) => serde_json::json!({
            "generation": generation,
            "revision": p.revision,
            "title": p.title,
            "width": p.width,
            "height": p.height,
        }),
        None => serde_json::json!({ "generation": generation }),
    };
    v.to_string()
}

async fn serve(mut stream: TcpStream, shared: Arc<Shared>) {
    let Some(text) = read_head(&mut stream).await else {
        return;
    };
    let Some(head) = parse_head(&text) else {
        let _ = respond(&mut stream, "400 Bad Request", "text/plain", b"", false).await;
        return;
    };
    let head_only = head.method == "HEAD";
    let result = match route(&head, &shared.token, shared.port) {
        Err(status) => {
            respond(
                &mut stream,
                status,
                "text/plain",
                status.as_bytes(),
                head_only,
            )
            .await
        }
        Ok(Route::Shell) => {
            respond(
                &mut stream,
                "200 OK",
                content_type("x.html"),
                SHELL.as_bytes(),
                head_only,
            )
            .await
        }
        Ok(Route::State) => {
            let page = shared.page.lock().expect("preview page lock").clone();
            let generation = *shared.generation.borrow();
            let body = state_json(page.as_deref(), generation);
            respond(
                &mut stream,
                "200 OK",
                "application/json",
                body.as_bytes(),
                head_only,
            )
            .await
        }
        Ok(Route::File(path)) => {
            let page = shared.page.lock().expect("preview page lock").clone();
            match page.as_ref().and_then(|p| p.files.get(path)) {
                Some(bytes) => {
                    respond(&mut stream, "200 OK", content_type(path), bytes, head_only).await
                }
                None => {
                    respond(
                        &mut stream,
                        "404 Not Found",
                        "text/plain",
                        b"not found",
                        head_only,
                    )
                    .await
                }
            }
        }
        Ok(Route::Events) => events(&mut stream, &shared).await,
    };
    if let Err(e) = result {
        log::debug!("preview connection: {e}");
    }
}

/// Counts a connected shell while it lives.
struct Client<'a>(&'a AtomicUsize);

impl Drop for Client<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn events(stream: &mut TcpStream, shared: &Shared) -> std::io::Result<()> {
    shared.clients.fetch_add(1, Ordering::SeqCst);
    let _client = Client(&shared.clients);
    let mut rx = shared.generation.subscribe();
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
              Cache-Control: no-store\r\nConnection: keep-alive\r\n\r\nretry: 1000\n\n",
        )
        .await?;
    // Read side: notice the browser closing the connection.
    let (mut read, mut write) = stream.split();
    let mut sink = [0u8; 256];
    loop {
        let g = *rx.borrow_and_update();
        write.write_all(format!("data: {g}\n\n").as_bytes()).await?;
        write.flush().await?;
        loop {
            tokio::select! {
                changed = rx.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                    break;
                }
                n = read.read(&mut sink) => {
                    if matches!(n, Ok(0) | Err(_)) {
                        return Ok(());
                    }
                }
                _ = tokio::time::sleep(HEARTBEAT) => {
                    write.write_all(b": ping\n\n").await?;
                    write.flush().await?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};

    fn head(path: &str, host: &str) -> Head {
        Head {
            method: "GET".into(),
            path: path.into(),
            host: Some(host.into()),
        }
    }

    #[test]
    fn parses_a_request_head() {
        let h =
            parse_head("GET /abc/page/index.html?g=3 HTTP/1.1\r\nHost: 127.0.0.1:9\r\nAccept: */*")
                .unwrap();
        assert_eq!(h, head("/abc/page/index.html", "127.0.0.1:9"));
    }

    #[test]
    fn routes_only_with_the_token_and_a_local_host() {
        let ok = "127.0.0.1:9";
        assert_eq!(route(&head("/tok/", ok), "tok", 9), Ok(Route::Shell));
        assert_eq!(route(&head("/tok/state", ok), "tok", 9), Ok(Route::State));
        assert_eq!(route(&head("/tok/events", ok), "tok", 9), Ok(Route::Events));
        assert_eq!(
            route(&head("/tok/page/assets/a.png", "localhost:9"), "tok", 9),
            Ok(Route::File("assets/a.png"))
        );
        assert_eq!(route(&head("/tok", ok), "tok", 9), Err("404 Not Found"));
        assert_eq!(route(&head("/other/", ok), "tok", 9), Err("404 Not Found"));
        assert_eq!(route(&head("/tokx/", ok), "tok", 9), Err("404 Not Found"));
        assert_eq!(
            route(&head("/tok/secret", ok), "tok", 9),
            Err("404 Not Found")
        );
        // DNS rebinding: a page on evil.example resolving to 127.0.0.1.
        assert_eq!(
            route(&head("/tok/", "evil.example:9"), "tok", 9),
            Err("403 Forbidden")
        );
        assert_eq!(
            route(&head("/tok/", "127.0.0.1:10"), "tok", 9),
            Err("403 Forbidden")
        );
        let mut post = head("/tok/", ok);
        post.method = "POST".into();
        assert_eq!(route(&post, "tok", 9), Err("405 Method Not Allowed"));
    }

    fn get(url: &str) -> (String, Vec<u8>) {
        let rest = url.strip_prefix("http://").unwrap();
        let (host, path) = rest.split_once('/').unwrap();
        let mut s = std::net::TcpStream::connect(host).unwrap();
        write!(s, "GET /{path} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        let end = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8(out[..end].to_vec()).unwrap();
        (head, out[end + 4..].to_vec())
    }

    #[test]
    fn serves_the_shell_the_page_and_reload_events() {
        let server = Server::start().unwrap();
        let url = server.url().to_string();
        assert!(url.starts_with("http://127.0.0.1:") && url.ends_with('/'));

        let (h, body) = get(&url);
        assert!(h.starts_with("HTTP/1.1 200"), "{h}");
        assert!(String::from_utf8(body).unwrap().contains("<iframe"));
        let (_, state) = get(&format!("{url}state"));
        assert_eq!(String::from_utf8(state).unwrap(), r#"{"generation":0}"#);
        let (h, _) = get(&format!("{url}page/index.html"));
        assert!(h.starts_with("HTTP/1.1 404"), "{h}");

        // An event stream sees the current generation, then each publish.
        let rest = url.strip_prefix("http://").unwrap();
        let (host, path) = rest.split_once('/').unwrap();
        let mut es = std::net::TcpStream::connect(host).unwrap();
        write!(es, "GET /{path}events HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
        let mut lines = BufReader::new(es.try_clone().unwrap()).lines();
        let mut next_data = || {
            lines
                .by_ref()
                .map(Result::unwrap)
                .find_map(|l| l.strip_prefix("data: ").map(str::to_string))
                .unwrap()
        };
        assert_eq!(next_data(), "0");
        for _ in 0..100 {
            if server.clients() == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(server.clients(), 1);

        let mut files = std::collections::HashMap::new();
        files.insert("index.html".to_string(), b"<p>hi</p>".to_vec());
        files.insert("assets/a.png".to_string(), vec![1, 2, 3]);
        server.publish(Page {
            revision: 4,
            title: "Home".into(),
            width: 390.0,
            height: 844.0,
            files,
        });
        assert_eq!(next_data(), "1");
        let (h, body) = get(&format!("{url}page/index.html"));
        assert!(h.contains("text/html"), "{h}");
        assert_eq!(body, b"<p>hi</p>");
        let (h, body) = get(&format!("{url}page/assets/a.png"));
        assert!(h.contains("image/png"), "{h}");
        assert_eq!(body, vec![1, 2, 3]);
        let (_, state) = get(&format!("{url}state"));
        let state: serde_json::Value = serde_json::from_slice(&state).unwrap();
        assert_eq!(state["generation"], 1);
        assert_eq!(state["title"], "Home");
        assert_eq!(state["width"], 390.0);

        es.shutdown(std::net::Shutdown::Both).unwrap();
        for _ in 0..200 {
            if server.clients() == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(server.clients(), 0);
    }
}
