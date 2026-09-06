//! E2E support: a real scratch hub (standing rule 9 — the real thing,
//! never a mock of the hub), a fake HA webhook server (HA cannot be
//! real in CI — what the fake cannot express is recorded in
//! TEST_PLAN.md), and a runner process handle.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener as StdTcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ── Scratch hub ─────────────────────────────────────────────────────────

enum HubProcess {
    Binary { child: Child },
    Docker { container: String },
}

pub struct Hub {
    process: Option<HubProcess>,
    pub port: u16,
    data_dir: Option<tempfile::TempDir>,
    binary: Option<PathBuf>,
    token: Option<String>,
    log_path: Option<PathBuf>,
}

/// Reserve a port ACROSS PROCESSES.
///
/// `cargo test --all` runs every suite as its own process, so an
/// in-process set is not enough: two suites probing `:0` at the same
/// moment can be handed the same port, and whichever binds second dies
/// with "Address already in use". That is exactly the flake that broke
/// two commit gates on 2026-08-30. The claim therefore lives in a
/// shared directory, where `create_new` is the atomic winner-takes-it
/// primitive every process can see.
pub fn reserve(port: u16) -> bool {
    use std::sync::OnceLock;
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    let dir = DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join("kyu-runner-test-ports");
        let _ = std::fs::create_dir_all(&dir);
        // Claims outlive their test run, so sweep the stale ones once
        // per process; without this the directory would slowly fill
        // with every ephemeral port the machine ever handed out.
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let stale = entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .map(|when| when.elapsed().unwrap_or_default() > Duration::from_secs(3600))
                    .unwrap_or(false);
                if stale {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        dir
    });
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(port.to_string()))
        .is_ok()
}

pub fn free_port() -> u16 {
    for _ in 0..200 {
        let port = StdTcpListener::bind("127.0.0.1:0")
            .expect("bind ephemeral")
            .local_addr()
            .expect("local addr")
            .port();
        if reserve(port) {
            return port;
        }
    }
    panic!("no free port could be reserved after 200 attempts");
}

fn hub_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("KYU_BIN") {
        // KYU_BIN="" forces the docker path even when the default
        // binary exists — how CI's environment is rehearsed locally.
        return (!path.is_empty()).then(|| PathBuf::from(path));
    }
    let default =
        PathBuf::from(std::env::var("HOME").ok()?).join("Projects/kyu/target/release/kyu");
    default.exists().then_some(default)
}

impl Hub {
    pub async fn start() -> Hub {
        Self::start_inner(None).await
    }

    /// A hub with the W2 door on: `token` is the master token scripts
    /// send as a bearer.
    pub async fn start_with_door(token: &str) -> Hub {
        Self::start_inner(Some(token.to_string())).await
    }

    async fn start_inner(token: Option<String>) -> Hub {
        let port = free_port();
        let data_dir = tempfile::TempDir::new().expect("hub data dir");
        let mut hub = Hub {
            process: None,
            port,
            data_dir: Some(data_dir),
            binary: hub_binary(),
            token,
            log_path: None,
        };
        hub.launch();
        hub.wait_ready().await;
        hub
    }

    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn launch(&mut self) {
        let listen = format!("127.0.0.1:{}", self.port);
        let data = self
            .data_dir
            .as_ref()
            .expect("data dir")
            .path()
            .to_path_buf();
        // 64 hex chars: the hub requires KYU_SECRET_KEY next to the
        // token; a fixed test key is fine — nothing real is protected.
        let secret_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let process = match &self.binary {
            Some(binary) => {
                let log = tempfile::NamedTempFile::new().expect("hub log file");
                let (log_file, log_path) = log.keep().expect("keep hub log");
                self.log_path = Some(log_path);
                let mut command = Command::new(binary);
                command
                    .env("KYU_LISTEN", &listen)
                    .env("KYU_DATA_DIR", &data)
                    .env("KYU_LOG", "warn")
                    .stdout(Stdio::null())
                    .stderr(log_file);
                if let Some(token) = &self.token {
                    command.env("KYU_TOKEN", token);
                    command.env("KYU_SECRET_KEY", secret_key);
                }
                let child = command
                    .spawn()
                    .expect("spawn kyu binary (set KYU_BIN or build ~/Projects/kyu)");
                HubProcess::Binary { child }
            }
            None => {
                let image = std::env::var("KYU_IMAGE")
                    // Pinned to the published 2.0.0 image (the hub the suite was written
                    // against); the kit CI has no KYU_BIN, so docker pulls this. 1.0.0 was
                    // never on GHCR — CI failed on it the first time the kit ran the suite.
                    .unwrap_or_else(|_| "ghcr.io/kennypassenier/kyu@sha256:13963aff9cb9b8449b19a51b209ef22661f7c174b1d9ddc8a88ef38240747177".into());
                let mut args: Vec<String> = vec![
                    "run".into(),
                    "-d".into(),
                    "-p".into(),
                    format!("127.0.0.1:{}:8080", self.port),
                    "-e".into(),
                    "KYU_LISTEN=0.0.0.0:8080".into(),
                    "-e".into(),
                    "KYU_LOG=warn".into(),
                ];
                if let Some(token) = &self.token {
                    args.push("-e".into());
                    args.push(format!("KYU_TOKEN={token}"));
                    args.push("-e".into());
                    args.push(format!("KYU_SECRET_KEY={secret_key}"));
                }
                args.push(image);
                let output = Command::new("docker")
                    .args(&args)
                    .output()
                    .expect("docker run (no KYU_BIN and no docker — one is required)");
                assert!(
                    output.status.success(),
                    "docker run failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let container = String::from_utf8_lossy(&output.stdout).trim().to_string();
                HubProcess::Docker { container }
            }
        };
        self.process = Some(process);
    }

    async fn wait_ready(&self) {
        let client = reqwest::Client::new();
        let url = format!("{}/healthz", self.base());
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Ok(response) = client.get(&url).send().await
                && response.status().is_success()
            {
                return;
            }
            // Loud, not silent: without the hub's own stderr a bind
            // clash reads as a mysterious timeout (2026-08-30 flake).
            assert!(
                Instant::now() < deadline,
                "hub did not become healthy on {url}. Its stderr said: {}",
                self.stderr()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn stderr(&self) -> String {
        match &self.log_path {
            Some(path) => std::fs::read_to_string(path).unwrap_or_default(),
            None => "(not captured)".into(),
        }
    }

    /// Stop the hub, keeping its data and port (K7 drill).
    pub fn stop(&mut self) {
        match self.process.take() {
            Some(HubProcess::Binary { mut child }) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            Some(HubProcess::Docker { container }) => {
                let _ = Command::new("docker")
                    .args(["rm", "-f", &container])
                    .output();
            }
            None => {}
        }
    }

    /// Start it again with a different door token (G5 drill): the hub's
    /// admission changed, the runner's token did not.
    pub async fn restart_with_token(&mut self, token: &str) {
        assert!(self.process.is_none(), "stop() first");
        assert!(
            self.binary.is_some(),
            "the token drill needs KYU_BIN (skipped under docker)"
        );
        self.token = Some(token.to_string());
        self.launch();
        self.wait_ready().await;
    }

    /// Start it again on the same port with the same data (K7 drill).
    pub async fn restart(&mut self) {
        assert!(self.process.is_none(), "stop() first");
        // The docker path loses /data on rm; bind the tempdir instead
        // would need matching uids — the binary path is the primary
        // restart vehicle. CI's suite skips restart drills when only
        // docker is available.
        assert!(
            self.binary.is_some(),
            "hub restart drill needs KYU_BIN (skipped under docker)"
        );
        self.launch();
        self.wait_ready().await;
    }

    pub fn supports_restart(&self) -> bool {
        self.binary.is_some()
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── Hub-side test verbs (the dashboard-documented API) ─────────────────

pub async fn publish(hub: &Hub, topic: &str, content_type: &str, body: &str) {
    publish_authed(hub, None, topic, content_type, body).await;
}

/// Raw publish: arbitrary bytes, optionally without a content-type.
pub async fn publish_bytes(hub: &Hub, topic: &str, content_type: Option<&str>, body: Vec<u8>) {
    let mut request = reqwest::Client::new()
        .post(format!("{}/t/{topic}", hub.base()))
        .body(body);
    if let Some(content_type) = content_type {
        request = request.header("content-type", content_type);
    }
    let response = request.send().await.expect("publish bytes");
    assert_eq!(response.status().as_u16(), 201, "publish should be 201");
}

pub async fn publish_authed(
    hub: &Hub,
    token: Option<&str>,
    topic: &str,
    content_type: &str,
    body: &str,
) {
    let mut request = reqwest::Client::new()
        .post(format!("{}/t/{topic}", hub.base()))
        .header("content-type", content_type)
        .body(body.to_string());
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.expect("publish");
    assert_eq!(response.status().as_u16(), 201, "publish should be 201");
}

/// Poll once with wait=0 as the given subscription; returns the body if
/// a message was pending. Used to prove acks (a drained queue answers
/// 204) and to pre-create subscriptions.
pub async fn poll_once(hub: &Hub, topic: &str, subscription: &str) -> Option<(String, String)> {
    poll_once_from(hub, topic, subscription, false).await
}

/// `from_beginning` replays retained history — needed when the test
/// publishes before the subscription's first poll (a subscription only
/// sees what follows its creation).
pub async fn poll_once_from(
    hub: &Hub,
    topic: &str,
    subscription: &str,
    from_beginning: bool,
) -> Option<(String, String)> {
    let from = if from_beginning {
        "&from=beginning"
    } else {
        ""
    };
    let response = reqwest::Client::new()
        .get(format!(
            "{}/t/{topic}/next?as={subscription}&wait=0{from}",
            hub.base()
        ))
        .send()
        .await
        .expect("poll");
    match response.status().as_u16() {
        204 | 404 => None,
        200 => {
            let id = response
                .headers()
                .get("kyu-id")
                .and_then(|value| value.to_str().ok())
                .expect("kyu-id")
                .to_string();
            let body = response.text().await.expect("body");
            Some((id, body))
        }
        other => panic!("poll answered {other}"),
    }
}

pub async fn ack(hub: &Hub, topic: &str, subscription: &str, id: &str) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/t/{topic}/ack/{id}?as={subscription}",
            hub.base()
        ))
        .send()
        .await
        .expect("ack");
    assert!(response.status().is_success(), "ack failed");
}

pub async fn put_policy(hub: &Hub, topic: &str, subscription: &str, policy: &str) {
    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/t/{topic}/subs/{subscription}/policy",
            hub.base()
        ))
        .header("content-type", "application/json")
        .body(policy.to_string())
        .send()
        .await
        .expect("put policy");
    assert!(
        response.status().is_success(),
        "policy PUT failed: {}",
        response.text().await.unwrap_or_default()
    );
}

/// The poison pill (kyu W5): straight to the dead-letter list —
/// the quickest way to make the hub emit a `message.dead_lettered`
/// event for the K6 test.
pub async fn nack_dead(hub: &Hub, topic: &str, subscription: &str, id: &str) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/t/{topic}/nack/{id}?as={subscription}&dead=true",
            hub.base()
        ))
        .send()
        .await
        .expect("nack dead");
    assert!(response.status().is_success(), "poison-pill nack failed");
}

pub async fn get_policy(hub: &Hub, topic: &str, subscription: &str) -> String {
    reqwest::Client::new()
        .get(format!(
            "{}/api/t/{topic}/subs/{subscription}/policy",
            hub.base()
        ))
        .send()
        .await
        .expect("get policy")
        .text()
        .await
        .expect("policy body")
}

/// True once the subscription exists on the hub (its policy endpoint
/// answers 200) — the deterministic "has the runner polled yet?" probe
/// for topics that already exist at hub start (e.g. kyu.events).
pub async fn subscription_exists(hub: &Hub, topic: &str, subscription: &str) -> bool {
    reqwest::Client::new()
        .get(format!(
            "{}/api/t/{topic}/subs/{subscription}/policy",
            hub.base()
        ))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

pub async fn dead_letters(hub: &Hub, topic: &str, subscription: &str) -> String {
    reqwest::Client::new()
        .get(format!(
            "{}/api/t/{topic}/subs/{subscription}/dead",
            hub.base()
        ))
        .send()
        .await
        .expect("dead letters")
        .text()
        .await
        .expect("dead letter body")
}

// ── Fake HA webhook server ──────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum HaMode {
    Ok200,
    Fail(u16),
    Redirect,
    /// Answer 200 after a delay — for kill/SIGTERM drills.
    SlowOk(Duration),
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub method: String,
    pub path: String,
    pub content_type: Option<String>,
    pub headers: Vec<(String, String)>,
    /// Raw bytes: K2's byte-for-byte promise includes payloads that
    /// are not UTF-8 (gap audit #4).
    pub body: Vec<u8>,
}

impl Hit {
    pub fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

pub struct FakeHa {
    pub port: u16,
    pub hits: Arc<Mutex<Vec<Hit>>>,
    mode: Arc<Mutex<HaMode>>,
    live: Arc<Mutex<bool>>,
}

impl FakeHa {
    pub fn start() -> FakeHa {
        // Bind the reserved port immediately: closing a probe listener
        // and rebinding later is the window that let two suites collide.
        let port = free_port();
        Self::start_on(port)
    }

    /// Binding a fixed port lets a test simulate "HA back after an
    /// outage" (AR15): drop one FakeHa, start another on the same port.
    /// The predecessor's socket may still be releasing, so retry
    /// briefly instead of failing the test over a millisecond.
    pub fn start_on(port: u16) -> FakeHa {
        let mut attempt = 0;
        let listener = loop {
            match StdTcpListener::bind(("127.0.0.1", port)) {
                Ok(listener) => break listener,
                Err(error) => {
                    attempt += 1;
                    assert!(attempt < 50, "fake HA could not bind {port}: {error}");
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        };
        listener.set_nonblocking(true).expect("nonblocking");
        let hits = Arc::new(Mutex::new(Vec::new()));
        let mode = Arc::new(Mutex::new(HaMode::Ok200));
        let live = Arc::new(Mutex::new(true));
        {
            let hits = Arc::clone(&hits);
            let mode = Arc::clone(&mode);
            let live = Arc::clone(&live);
            std::thread::spawn(move || {
                loop {
                    if !*live.lock().unwrap() {
                        return;
                    }
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let hits = Arc::clone(&hits);
                            let mode = mode.lock().unwrap().clone();
                            std::thread::spawn(move || handle(stream, hits, mode));
                        }
                        Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => return,
                    }
                }
            });
        }
        FakeHa {
            port,
            hits,
            mode,
            live,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub fn set_mode(&self, mode: HaMode) {
        *self.mode.lock().unwrap() = mode;
    }

    pub fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }

    /// Take the server down; the port stays reserved for a successor.
    pub fn shut_down(self) -> u16 {
        *self.live.lock().unwrap() = false;
        self.port
    }
}

fn handle(mut stream: std::net::TcpStream, hits: Arc<Mutex<Vec<Hit>>>, mode: HaMode) {
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    let header_end = loop {
        match stream.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => {
                raw.extend_from_slice(&buffer[..read]);
                if let Some(position) = find_header_end(&raw) {
                    break position;
                }
            }
            Err(_) => return,
        }
    };
    let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut content_type = None;
    let mut content_length = 0usize;
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-type" {
                content_type = Some(value.clone());
            }
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    let mut body = raw[header_end + 4..].to_vec();
    while body.len() < content_length {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => body.extend_from_slice(&buffer[..read]),
            Err(_) => break,
        }
    }
    hits.lock().unwrap().push(Hit {
        method,
        path,
        content_type,
        headers,
        body,
    });
    let response = match mode {
        HaMode::Ok200 => {
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string()
        }
        HaMode::Fail(status) => {
            format!("HTTP/1.1 {status} NO\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
        }
        HaMode::Redirect => "HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:1/elsewhere\r\n\
             content-length: 0\r\nconnection: close\r\n\r\n"
            .to_string(),
        HaMode::SlowOk(delay) => {
            std::thread::sleep(delay);
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string()
        }
    };
    let _ = stream.write_all(response.as_bytes());
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

// ── The runner under test ───────────────────────────────────────────────

pub struct Runner {
    pub child: Child,
    pub log_path: PathBuf,
    _config: tempfile::NamedTempFile,
    _state: tempfile::TempDir,
}

impl Runner {
    pub fn start(config_text: &str) -> Runner {
        Self::start_with_env(config_text, &[])
    }

    pub fn start_with_env(config_text: &str, env: &[(&str, &str)]) -> Runner {
        // Since the chassis migration the observation socket is the kit's
        // and always listens: a test that used to opt in with
        // `healthz_listen = "..."` in the config now gets that address as
        // `--listen`; every other test listens on an ephemeral port.
        let mut listen = "127.0.0.1:0".to_string();
        let mut kept = Vec::new();
        for line in config_text.lines() {
            if let Some(rest) = line.trim().strip_prefix("healthz_listen") {
                if let Some(addr) = rest.split('"').nth(1) {
                    listen = addr.to_string();
                }
                continue;
            }
            kept.push(line);
        }
        let config_text = kept.join("\n");
        let mut config = tempfile::NamedTempFile::new().expect("config file");
        config
            .write_all(config_text.as_bytes())
            .expect("write config");
        let state = tempfile::tempdir().expect("state dir");
        let log = tempfile::NamedTempFile::new().expect("log file");
        let (log_file, log_path) = log.keep().expect("keep log");
        // KYU_RUNNER_BIN lets the release workflow run this suite against
        // the musl artifact — the shipped binary is the tested binary
        // (T8/M1).
        let binary = std::env::var("KYU_RUNNER_BIN")
            .unwrap_or_else(|_| env!("CARGO_BIN_EXE_kyu-runner").into());
        let mut command = Command::new(binary);
        command
            .arg("--config")
            .arg(config.path())
            .arg("--state-dir")
            .arg(state.path())
            .arg("--listen")
            .arg(&listen)
            // The kit's shutdown bound wraps the pump's own grace (AR9):
            // wide enough for the slowest delivery any test configures.
            .args(["--shutdown-timeout-ms", "20000"])
            // Crate-scoped debug: hyper/reqwest connect spam would break
            // the K7 log-volume bound and drown the assertions.
            .env("KYU_RUNNER_LOG", "info,kyu_runner=debug")
            .stdout(Stdio::null())
            .stderr(log_file);
        for (name, value) in env {
            // The hub token has its own name since 0.2.0 (KYU_RUNNER_TOKEN
            // is the kit's dashboard login token); tests keep the old name.
            if *name == "KYU_RUNNER_TOKEN" {
                command.env("KYU_RUNNER_HUB_TOKEN", value);
            } else {
                command.env(name, value);
            }
        }
        let child = command.spawn().expect("spawn runner");
        Runner {
            child,
            log_path,
            _config: config,
            _state: state,
        }
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }

    pub fn kill_hard(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn sigterm(&self) {
        self.signal("-TERM");
    }

    pub fn signal(&self, signal: &str) {
        let _ = Command::new("kill")
            .args([signal, &self.child.id().to_string()])
            .output();
    }

    pub fn wait_exit(&mut self, within: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.kill_hard();
        let _ = std::fs::remove_file(&self.log_path);
    }
}

// ── Small helpers ───────────────────────────────────────────────────────

pub fn route_config(hub: &Hub, name: &str, topic: &str, webhook_url: &str) -> String {
    format!(
        "hub_url = \"{}\"\n\n[[routes]]\nname = \"{name}\"\ntopic = \"{topic}\"\n\
         subscription = \"ha-runner\"\nwebhook_url = \"{webhook_url}\"\n",
        hub.base()
    )
}

/// Wait until the runner has polled a not-yet-born topic once (the
/// AR16 log line). Publishing before the runner's first poll would
/// create the topic under the runner's feet and the subscription would
/// start "from now" — the documented boundary the runbook orders
/// around ("route first, then producers").
pub async fn wait_first_poll(runner: &Runner) {
    let log = || runner.log();
    wait_until(
        "the runner's first poll (topic not born line)",
        Duration::from_secs(15),
        || log().contains("topic not born"),
    )
    .await;
}

pub async fn wait_until<F: FnMut() -> bool>(what: &str, within: Duration, mut check: F) {
    let deadline = Instant::now() + within;
    loop {
        if check() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
