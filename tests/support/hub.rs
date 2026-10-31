//! The scratch hub as a test handle: start, stop, restart drills.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::hub_launch::{self, ADMIN_TOKEN, HubProcess};
use super::ports::free_port;

/// How long a freshly launched hub gets to answer `/healthz`.
const READY_TIMEOUT: Duration = Duration::from_secs(120);

/// Pause between two `/healthz` probes.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub struct Hub {
    process: Option<HubProcess>,
    pub port: u16,
    data_dir: Option<tempfile::TempDir>,
    binary: Option<PathBuf>,
    client_token: Option<String>,
    log_path: Option<PathBuf>,
}

impl Hub {
    /// A scratch hub with its door on — the only shape kyu 3.0.0 has.
    /// It starts with an admin token and issues this handle a client
    /// token for the three verbs.
    pub async fn start() -> Hub {
        let port = free_port();
        let data_dir = tempfile::TempDir::new().expect("hub data dir");
        let mut hub = Hub {
            process: None,
            port,
            data_dir: Some(data_dir),
            binary: hub_launch::hub_binary(),
            log_path: None,
            client_token: None,
        };
        hub.launch();
        hub.wait_ready().await;
        hub.mint_client_token().await;
        hub
    }

    /// The bearer every caller of the three verbs needs since kyu 3.0.0:
    /// a *client* token, issued the way the Clients page and `chassis
    /// clients issue` do it. Before 3.0.0 a hub without `KYU_TOKEN` had no
    /// door at all and the suite published anonymously; that state no
    /// longer exists (measured 2026-09-09: publishing without a bearer
    /// answers 401).
    ///
    /// The issue-and-reveal pair was thirty hand-written lines here until
    /// chassis 2.0.0 gave `AdminApi` — the clients API of *another* kit
    /// service, which is exactly this case: the hub is a chassis service
    /// and this is a headless caller of it.
    async fn mint_client_token(&mut self) {
        let api = chassis::admin::AdminApi::new(&self.base(), self.admin_token())
            .expect("build the hub's admin API client");
        let issued = api
            .issue_client("kyu-runner-suite", &[])
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "the hub refused to issue a client token: {e}. Its stderr said: {}",
                    self.stderr()
                )
            });
        self.client_token = Some(issued.token);
    }

    /// The admin token the hub is started with: the operator's secret,
    /// used here to mint client tokens and to reach the `/api/…`
    /// management routes.
    pub fn admin_token(&self) -> &str {
        ADMIN_TOKEN
    }

    /// The bearer the runner and the test verbs send on the three verbs.
    pub fn client_token(&self) -> &str {
        self.client_token
            .as_deref()
            .expect("the hub minted a client token at start")
    }

    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn launch(&mut self) {
        let listen = format!("127.0.0.1:{}", self.port);
        let process = match &self.binary {
            Some(binary) => {
                let data = self.data_dir.as_ref().expect("data dir").path();
                let (process, log_path) = hub_launch::spawn_binary(binary, &listen, data);
                self.log_path = Some(log_path);
                process
            }
            None => hub_launch::spawn_docker(self.port),
        };
        self.process = Some(process);
    }

    async fn wait_ready(&self) {
        let client = reqwest::Client::new();
        let url = format!("{}/healthz", self.base());
        let deadline = Instant::now() + READY_TIMEOUT;
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
            tokio::time::sleep(READY_POLL_INTERVAL).await;
        }
    }

    fn stderr(&self) -> String {
        match &self.log_path {
            // An unreadable log is reported inline: it is only ever
            // read to explain a failure that is already being raised.
            Some(path) => std::fs::read_to_string(path)
                .unwrap_or_else(|e| format!("(log {} unreadable: {e})", path.display())),
            None => "(not captured)".into(),
        }
    }

    /// Stop the hub, keeping its data and port (K7 drill).
    pub fn stop(&mut self) {
        if let Some(process) = self.process.take() {
            process.terminate();
        }
    }

    /// Start again on the same port with an EMPTY store, so the client
    /// token this hub issued is no longer known and its holder is denied
    /// (K9/G5 drill). Returns the original store; hand it back to
    /// [`Hub::restore_clients`] to make that same token valid again.
    ///
    /// Before kyu 3.0.0 this drill simply restarted the hub with another
    /// master token. A client token cannot be chosen that way — the hub
    /// generates it — so what moves here is the store behind the door,
    /// not the token in front of it. The runner's token never changes,
    /// which is the property under test.
    pub async fn restart_without_clients(&mut self) -> tempfile::TempDir {
        assert!(self.process.is_none(), "stop() first");
        assert!(
            self.binary.is_some(),
            "the token drill needs KYU_BIN (skipped under docker)"
        );
        let known = self.data_dir.take().expect("data dir");
        self.data_dir = Some(tempfile::TempDir::new().expect("empty hub store"));
        self.launch();
        self.wait_ready().await;
        known
    }

    /// Put the original store back: the token from before is accepted
    /// again, without the runner having restarted.
    pub async fn restore_clients(&mut self, known: tempfile::TempDir) {
        assert!(self.process.is_none(), "stop() first");
        self.data_dir = Some(known);
        self.launch();
        self.wait_ready().await;
    }

    /// Start it again on the same port with the same data (K7 drill).
    pub async fn restart(&mut self) {
        assert!(self.process.is_none(), "stop() first");
        // The docker path loses /data on rm; bind the tempdir instead
        // would need matching uids — the binary path is the primary
        // restart vehicle. The suite skips restart drills when only
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
