//! `tessel mcp`: the stdio bridge Claude Code starts (§6.1). It
//! connects to a running window's socket and copies newline-delimited
//! JSON-RPC both ways. It understands nothing about MCP except when no app
//! is running: then it answers `initialize` itself and fails every tool
//! call with `not_running`, trying the socket again on each request so a
//! window started later is picked up (the client's `initialize` is replayed
//! to it). With `--launch` the app is started lazily, by the first request
//! that needs a window.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent::{ErrorCode, ToolError};
use serde_json::{json, Value};

use crate::agent_socket::{self, CURRENT};

pub const USAGE: &str = "usage: tessel mcp [--socket PATH | --pid PID | --file PATH] [--launch]

Bridges MCP over stdio to a running Tessel window. Register it with:
  claude mcp add tessel -- tessel mcp

  --socket PATH  connect to this socket
  --pid PID      connect to the window of this process
  --file PATH    connect to the window that has this document open
  --launch       start Tessel (with --file, on that file) when the
                 agent first needs a window and none is running (not at
                 startup), and wait for it";

/// How long `--launch` waits for the new window's socket.
const LAUNCH_WAIT: Duration = Duration::from_secs(30);

/// The id of the replayed `initialize`, whose response is not forwarded.
const REPLAY_ID: &str = "tessel-bridge-initialize";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    pub socket: Option<PathBuf>,
    pub pid: Option<u32>,
    pub file: Option<PathBuf>,
    pub launch: bool,
    /// Where windows put their sockets.
    pub dir: PathBuf,
}

/// Parse the arguments after `mcp`. `Ok(None)` for `--help`.
pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Option<Options>, String> {
    let mut opts = Options {
        dir: agent_socket::runtime_dir(),
        ..Default::default()
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.to_str() {
            Some("-h" | "--help") => return Ok(None),
            Some("--socket") => opts.socket = Some(value("--socket")?.into()),
            Some("--file") => opts.file = Some(value("--file")?.into()),
            Some("--pid") => {
                let v = value("--pid")?;
                let pid = v
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| format!("--pid {v:?} is not a process id"))?;
                opts.pid = Some(pid);
            }
            Some("--launch") => opts.launch = true,
            _ => return Err(format!("unexpected argument {arg:?}")),
        }
    }
    Ok(Some(opts))
}

/// Whether sockets in `dir` may be trusted: `Ok(false)` if it does not
/// exist (no window yet), an error if another user could have planted a
/// socket in it (see [`agent_socket::check_private_dir`]).
fn trusted_dir(dir: &Path) -> Result<bool, String> {
    match agent_socket::check_private_dir(dir) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

/// The window to connect to, if one is running. Fails if the socket's
/// directory is not private to this user.
pub fn connect(opts: &Options) -> Result<Option<UnixStream>, String> {
    if let Some(p) = &opts.socket {
        let dir = match p.parent() {
            Some(d) if !d.as_os_str().is_empty() => d,
            _ => Path::new("."),
        };
        return Ok(trusted_dir(dir)?
            .then(|| UnixStream::connect(p).ok())
            .flatten());
    }
    if !trusted_dir(&opts.dir)? {
        return Ok(None);
    }
    Ok(connect_in_dir(opts))
}

fn connect_in_dir(opts: &Options) -> Option<UnixStream> {
    if let Some(pid) = opts.pid {
        return UnixStream::connect(agent_socket::socket_path(&opts.dir, pid)).ok();
    }
    let live: Vec<u32> = agent_socket::socket_pids(&opts.dir)
        .into_iter()
        .filter(|p| agent_socket::pid_alive(*p))
        .collect();
    if let Some(file) = &opts.file {
        let want = file.canonicalize().unwrap_or_else(|_| file.clone());
        return live
            .iter()
            .filter(|p| agent_socket::window_file(&opts.dir, **p).as_ref() == Some(&want))
            .find_map(|p| UnixStream::connect(agent_socket::socket_path(&opts.dir, *p)).ok());
    }
    UnixStream::connect(opts.dir.join(CURRENT))
        .ok()
        .or_else(|| {
            // The newest window first.
            live.iter()
                .rev()
                .find_map(|p| UnixStream::connect(agent_socket::socket_path(&opts.dir, *p)).ok())
        })
}

/// Start the app detached (not a child of the agent's process group) and
/// wait for its socket.
pub fn launch(opts: &Options) -> io::Result<UnixStream> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    if let Some(f) = &opts.file {
        cmd.arg(f);
    }
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()?;
    let path = agent_socket::socket_path(&opts.dir, child.id());
    let deadline = Instant::now() + LAUNCH_WAIT;
    loop {
        match trusted_dir(&opts.dir) {
            Ok(true) => {
                if let Ok(s) = UnixStream::connect(&path) {
                    return Ok(s);
                }
            }
            Ok(false) => {}
            Err(why) => {
                // The new window cannot listen there either.
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, why));
            }
        }
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "Tessel exited before opening its socket ({status})"
            )));
        }
        if Instant::now() > deadline {
            // Do not leave a slow window behind for the next launch to double.
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("Tessel did not open {}", path.display()),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

type Out = Arc<Mutex<Box<dyn Write + Send>>>;

fn write_line(out: &Out, line: &str) {
    let mut w = out.lock().expect("stdout lock");
    let _ = w.write_all(line.as_bytes());
    if !line.ends_with('\n') {
        let _ = w.write_all(b"\n");
    }
    let _ = w.flush();
}

/// `launch` is whether `--launch` is on; `failure` why connecting or the
/// launch just failed.
fn not_running_error(launch: bool, failure: Option<&str>) -> ToolError {
    let message = match (launch, failure) {
        (_, Some(why)) => format!("Tessel is not available: {why}"),
        (true, None) => "Tessel is not running; it is started on the next tool call".into(),
        (false, None) => "Tessel is not running; start it, or pass --launch".into(),
    };
    ToolError::new(ErrorCode::NotRunning, message)
}

/// The answer to a request no window can take: a tool error for
/// `tools/call`, a JSON-RPC error otherwise.
fn not_running_reply(id: &Value, method: &str, err: ToolError) -> Value {
    if method == "tools/call" {
        json!({ "jsonrpc": "2.0", "id": id, "result": {
            "content": [{ "type": "text", "text": err.to_json() }],
            "isError": true
        } })
    } else {
        json!({ "jsonrpc": "2.0", "id": id,
            "error": { "code": -32603, "message": err.message } })
    }
}

/// Whether a message needs a window to answer: a request `local_reply`
/// would fail with `not_running`. Only these may launch the app.
fn needs_window(msg: &Value) -> bool {
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return false;
    };
    msg.get("id").is_some()
        && !matches!(
            method,
            "initialize"
                | "ping"
                | "tools/list"
                | "resources/list"
                | "resources/templates/list"
                | "prompts/list"
                | "logging/setLevel"
        )
}

/// Answer a request ourselves (no window is running). `launch` is whether
/// the bridge starts the app on demand.
fn local_reply(msg: &Value, launch: bool, failure: Option<&str>) -> Option<Value> {
    let id = msg.get("id")?;
    let method = msg.get("method")?.as_str()?;
    let result = match method {
        "initialize" => {
            let version = msg["params"]["protocolVersion"]
                .as_str()
                .unwrap_or("2025-06-18");
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "tessel", "version": env!("CARGO_PKG_VERSION") },
                "instructions": if launch {
                    "Tessel is not running yet; it opens on the first tool call."
                } else {
                    "Tessel is not running. Ask the user to start it (or \
                    register the bridge with --launch); tool calls fail with not_running until then."
                }
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": agent::tools::list() }),
        "resources/list" => json!({ "resources": [] }),
        "resources/templates/list" => json!({ "resourceTemplates": [] }),
        "prompts/list" => json!({ "prompts": [] }),
        "logging/setLevel" => json!({}),
        _ => {
            let err = not_running_error(launch, failure);
            return Some(not_running_reply(id, method, err));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// Requests sent to the window and not answered yet: id → method.
type InFlight = Arc<Mutex<BTreeMap<String, String>>>;

/// A connection to a window.
struct Conn {
    stream: UnixStream,
    in_flight: InFlight,
    closed: Arc<std::sync::atomic::AtomicBool>,
}

impl Conn {
    /// Start copying the window's lines to `out`. When the window goes
    /// away, requests still in flight are answered with `not_running`.
    fn start(stream: UnixStream, out: Out, launch: bool) -> io::Result<Conn> {
        let reader = stream.try_clone()?;
        let in_flight = InFlight::default();
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (flight, done) = (in_flight.clone(), closed.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                let msg: Option<Value> = serde_json::from_str(&line).ok();
                let id = msg.as_ref().and_then(|m| m.get("id")).map(Value::to_string);
                if let Some(id) = &id {
                    if id == &Value::from(REPLAY_ID).to_string() {
                        continue;
                    }
                    flight.lock().expect("in-flight lock").remove(id);
                }
                write_line(&out, &line);
            }
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            let orphans = std::mem::take(&mut *flight.lock().expect("in-flight lock"));
            for (id, method) in orphans {
                let id: Value = serde_json::from_str(&id).unwrap_or(Value::Null);
                let err = ToolError::new(
                    ErrorCode::NotRunning,
                    if launch {
                        "Tessel closed; it is started again on the next tool call"
                    } else {
                        "Tessel closed; start it, or pass --launch"
                    },
                );
                write_line(&out, &not_running_reply(&id, &method, err).to_string());
            }
        });
        Ok(Conn {
            stream,
            in_flight,
            closed,
        })
    }

    fn is_open(&self) -> bool {
        !self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn send(&mut self, line: &str) -> io::Result<()> {
        self.stream.write_all(line.as_bytes())?;
        if !line.ends_with('\n') {
            self.stream.write_all(b"\n")?;
        }
        self.stream.flush()
    }
}

/// Run the bridge until `input` ends. `connect` finds a window each time
/// one is needed; its argument says whether it may launch the app (only for
/// a request that needs a window) and fails with the reason connecting or
/// a launch did.
/// `launch` is whether `--launch` is on.
pub fn serve(
    input: impl BufRead,
    output: Box<dyn Write + Send>,
    launch: bool,
    mut connect: impl FnMut(bool) -> Result<Option<UnixStream>, String>,
) {
    let out: Out = Arc::new(Mutex::new(output));
    let mut conn: Option<Conn> = None;
    // The client's `initialize`, to replay to a window found later.
    let mut init: Option<Value> = None;
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Option<Value> = serde_json::from_str(&line).ok();
        let method = msg
            .as_ref()
            .and_then(|m| m.get("method"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let is_init = method == "initialize";
        if is_init {
            init = msg.clone();
        }
        if conn.as_ref().is_some_and(|c| !c.is_open()) {
            conn = None;
        }
        let mut failure = None;
        if conn.is_none() && (is_init || init.is_some()) {
            let may_launch = launch && msg.as_ref().is_some_and(needs_window);
            let found = connect(may_launch).unwrap_or_else(|why| {
                failure = Some(why);
                None
            });
            conn = found.and_then(|s| Conn::start(s, out.clone(), launch).ok());
            if let (Some(c), false, Some(init)) = (conn.as_mut(), is_init, &init) {
                // A window appeared after the client initialized with us.
                let mut replay = init.clone();
                replay["id"] = json!(REPLAY_ID);
                let initialized =
                    json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
                if c.send(&replay.to_string())
                    .and_then(|_| c.send(&initialized.to_string()))
                    .is_err()
                {
                    conn = None;
                }
            }
        }
        if let Some(c) = conn.as_mut() {
            if let Some(id) = msg.as_ref().and_then(|m| m.get("id")) {
                if !method.is_empty() {
                    c.in_flight
                        .lock()
                        .expect("in-flight lock")
                        .insert(id.to_string(), method.clone());
                }
            }
            if c.send(&line).is_ok() {
                continue;
            }
            if let Some(id) = msg.as_ref().and_then(|m| m.get("id")) {
                c.in_flight
                    .lock()
                    .expect("in-flight lock")
                    .remove(&id.to_string());
            }
            conn = None;
        }
        if let Some(reply) = msg
            .as_ref()
            .and_then(|m| local_reply(m, launch, failure.as_deref()))
        {
            write_line(&out, &reply.to_string());
        }
    }
    if let Some(c) = conn {
        let _ = c.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// `tessel mcp …`
pub fn main(args: impl IntoIterator<Item = OsString>) -> std::process::ExitCode {
    let opts = match parse_args(args) {
        Ok(Some(o)) => o,
        Ok(None) => {
            // stdout belongs to MCP only when serving; --help may use it.
            println!("{USAGE}");
            return std::process::ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("tessel mcp: {e}\n\n{USAGE}");
            return std::process::ExitCode::from(2);
        }
    };
    let stdin = io::stdin().lock();
    serve(stdin, Box::new(io::stdout()), opts.launch, |may_launch| {
        let found = connect(&opts).inspect_err(|e| log::error!("{e}"))?;
        if found.is_some() || !may_launch {
            return Ok(found);
        }
        launch(&opts).map(Some).map_err(|e| {
            log::error!("--launch: {e}");
            format!("it could not be started: {e}")
        })
    });
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;
    use std::path::Path;

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Sink {
        fn messages(&self) -> Vec<Value> {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }
    }

    #[test]
    fn without_a_window_it_initializes_and_fails_tool_calls() {
        let input = [
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": { "name": "claude-code", "version": "1" } } }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "get_document", "arguments": {} } }),
            json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/read",
                "params": { "uri": "design://document" } }),
        ]
        .iter()
        .map(|v| format!("{v}\n"))
        .collect::<String>();
        let sink = Sink::default();
        let mut tries = 0;
        serve(
            io::Cursor::new(input),
            Box::new(sink.clone()),
            false,
            |_| {
                tries += 1;
                Ok(None)
            },
        );
        let out = sink.messages();
        assert_eq!(out.len(), 4, "{out:?}");
        assert_eq!(out[0]["id"], 1);
        assert_eq!(out[0]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(out[0]["result"]["serverInfo"]["name"], "tessel");
        assert_eq!(out[1]["result"]["tools"].as_array().unwrap().len(), 20);
        assert_eq!(out[2]["result"]["isError"], true);
        let body: Value =
            serde_json::from_str(out[2]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["code"], "not_running");
        assert!(out[3]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not running"));
        assert_eq!(
            tries, 5,
            "it looks for a window on every message from initialize on"
        );
    }

    /// Input that ends only once `sink` holds `n` lines, so a window's
    /// reply is not cut off by the bridge shutting down.
    struct UntilReplies(Sink, usize);

    impl io::Read for UntilReplies {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.0.messages().len() < self.1 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(0)
        }
    }

    fn session(msgs: &[Value]) -> String {
        msgs.iter().map(|v| format!("{v}\n")).collect()
    }

    fn handshake() -> Vec<Value> {
        vec![
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": { "name": "claude-code", "version": "1" } } }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" }),
            json!({ "jsonrpc": "2.0", "id": 4, "method": "prompts/list" }),
            json!({ "jsonrpc": "2.0", "id": 5, "method": "ping" }),
            json!({ "jsonrpc": "2.0", "id": 6, "method": "resources/templates/list" }),
            json!({ "jsonrpc": "2.0", "id": 7, "method": "logging/setLevel",
                "params": { "level": "info" } }),
        ]
    }

    #[test]
    fn launch_waits_for_a_request_that_needs_a_window() {
        let mut msgs = handshake();
        msgs.push(json!({ "jsonrpc": "2.0", "id": 8, "method": "tools/call",
            "params": { "name": "get_document", "arguments": {} } }));
        let (bridge_end, window_end) = UnixStream::pair().unwrap();
        // The fake window records what it receives and answers each request.
        let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
        let log = seen.clone();
        let mut w = window_end.try_clone().unwrap();
        let window = std::thread::spawn(move || {
            for line in BufReader::new(window_end).lines() {
                let msg: Value = serde_json::from_str(&line.unwrap()).unwrap();
                if let Some(id) = msg.get("id") {
                    let reply =
                        json!({ "jsonrpc": "2.0", "id": id, "result": { "from": "window" } });
                    writeln!(w, "{reply}").unwrap();
                }
                log.lock().unwrap().push(msg);
            }
        });
        let sink = Sink::default();
        let mut window_conn = Some(bridge_end);
        let mut launches = 0;
        let mut after_list = None;
        let mut calls = 0;
        serve(
            BufReader::new(io::Cursor::new(session(&msgs)).chain(UntilReplies(sink.clone(), 8))),
            Box::new(sink.clone()),
            true,
            |may_launch| {
                calls += 1;
                if may_launch {
                    launches += 1;
                    after_list = Some(calls);
                    return Ok(window_conn.take());
                }
                Ok(None)
            },
        );
        window.join().unwrap();
        assert_eq!(launches, 1);
        assert_eq!(after_list, Some(calls), "the launch is on the tools/call");
        let out = sink.messages();
        assert_eq!(out.len(), 8, "{out:?}");
        assert!(out[0]["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("first tool call"));
        assert_eq!(out[7]["id"], 8);
        assert_eq!(out[7]["result"]["from"], "window");
        let seen = seen.lock().unwrap();
        let methods: Vec<_> = seen.iter().map(|m| m["method"].as_str().unwrap()).collect();
        assert_eq!(
            methods,
            ["initialize", "notifications/initialized", "tools/call"]
        );
        assert_eq!(seen[0]["id"], REPLAY_ID);
    }

    #[test]
    fn launch_does_not_start_on_the_handshake_or_lists() {
        let sink = Sink::default();
        let mut launches = 0;
        serve(
            io::Cursor::new(session(&handshake())),
            Box::new(sink.clone()),
            true,
            |may_launch| {
                launches += usize::from(may_launch);
                Ok(None)
            },
        );
        assert_eq!(launches, 0);
        assert_eq!(sink.messages().len(), 7);
    }

    #[test]
    fn a_failed_launch_is_not_running_and_is_tried_again() {
        let mut msgs = handshake();
        for id in [8, 9] {
            msgs.push(json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": { "name": "get_document", "arguments": {} } }));
        }
        msgs.push(
            json!({ "jsonrpc": "2.0", "id": 10, "method": "resources/read",
            "params": { "uri": "design://document" } }),
        );
        let sink = Sink::default();
        let mut launches = 0;
        serve(
            io::Cursor::new(session(&msgs)),
            Box::new(sink.clone()),
            true,
            |may_launch| {
                launches += usize::from(may_launch);
                Err("boom".to_string())
            },
        );
        assert_eq!(launches, 3);
        let out = sink.messages();
        assert_eq!(out.len(), 10, "{out:?}");
        assert_eq!(out[7]["result"]["isError"], true);
        assert_eq!(out[8]["result"]["isError"], true);
        assert!(out[7]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not available: boom"));
        assert!(out[9]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not available: boom"));
    }

    #[test]
    fn connect_refuses_socket_directories_other_users_could_plant_in() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("god-bridge-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("tessel");
        let opts = Options {
            dir: dir.clone(),
            ..Default::default()
        };
        assert!(connect(&opts).unwrap().is_none(), "no directory, no window");

        std::fs::create_dir_all(&dir).unwrap();
        let chmod = |m| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(m)).unwrap();
        chmod(0o700);
        let sock = agent_socket::socket_path(&dir, std::process::id());
        let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        assert!(connect(&Options {
            pid: Some(std::process::id()),
            ..opts.clone()
        })
        .unwrap()
        .is_some());
        let explicit = Options {
            socket: Some(sock.clone()),
            ..opts.clone()
        };
        assert!(connect(&explicit).unwrap().is_some());

        chmod(0o755);
        let e = connect(&opts).unwrap_err();
        assert!(e.contains("refusing socket directory"), "{e}");
        let e = connect(&explicit).unwrap_err();
        assert!(
            e.contains("refusing socket directory"),
            "--socket's parent: {e}"
        );
        let link = root.join("link");
        chmod(0o700);
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(connect(&Options {
            dir: link,
            ..opts.clone()
        })
        .is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn arguments() {
        let parse = |a: &[&str]| parse_args(a.iter().map(OsString::from));
        let o = parse(&["--socket", "/s", "--launch"]).unwrap().unwrap();
        assert_eq!(o.socket.as_deref(), Some(Path::new("/s")));
        assert!(o.launch);
        assert_eq!(parse(&["--pid", "42"]).unwrap().unwrap().pid, Some(42));
        assert!(parse(&["--pid", "x"]).is_err());
        assert!(parse(&["--file"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert_eq!(parse(&["--help"]).unwrap(), None);
    }
}
