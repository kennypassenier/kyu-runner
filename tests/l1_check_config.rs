// L1 · W2 (--check-config) + K8 (fail-closed startup) at the binary
// boundary: real process, real exit codes, remedies on stderr.

use std::io::Write;
use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    // BRIDGE_BIN lets the release workflow run this suite against the
    // musl artifact (T8/M1).
    let binary =
        std::env::var("BRIDGE_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_hub-bridge").into());
    Command::new(binary)
        .args(args)
        .output()
        .expect("binary runs")
}

fn config_file(text: &str) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("tempfile");
    file.write_all(text.as_bytes()).expect("write config");
    file
}

const VALID: &str = r#"
hub_url = "http://127.0.0.1:8080"

[[routes]]
name = "mailbox-events"
topic = "mailbox.events"
subscription = "ha-bridge"
webhook_url = "http://ha.lan:8123/api/webhook/hub_mailbox_events"
"#;

#[test]
fn l1_w2_check_config_exits_zero_on_a_valid_config() {
    let file = config_file(VALID);
    let out = run(&["--config", file.path().to_str().unwrap(), "--check-config"]);
    assert!(out.status.success(), "stderr: {:?}", out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("config OK: 1 route(s)"), "{stdout}");
}

#[test]
fn l1_w2_check_config_fails_with_a_remedy_on_an_invalid_config() {
    let file = config_file(&VALID.replace("hub_url", "hub_uri"));
    let out = run(&["--config", file.path().to_str().unwrap(), "--check-config"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Remedy:"), "{stderr}");
}

#[test]
fn l1_k8_a_missing_config_file_fails_with_a_remedy() {
    let out = run(&["--config", "/nonexistent/hub-bridge.toml", "--check-config"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Remedy:"), "{stderr}");
}

#[test]
fn l1_w2_an_unknown_argument_fails_with_a_remedy() {
    let out = run(&["--frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--help"), "{stderr}");
}

#[test]
fn l1_w2_version_prints_the_crate_version() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")), "{stdout}");
}

#[test]
fn l1_w2_check_config_makes_no_network_calls() {
    // Gap audit #15: --check-config is wired as ExecStartPre= on a
    // booting LXC where the hub may not be up yet — a pre-flight call
    // would break every cold boot. Point the config at a listener we
    // hold and prove nothing ever connects.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    listener.set_nonblocking(true).expect("nonblocking");

    let config = VALID.replace("http://127.0.0.1:8080", &format!("http://{addr}"));
    let file = config_file(&config);
    let out = run(&["--config", file.path().to_str().unwrap(), "--check-config"]);
    assert!(out.status.success(), "stderr: {:?}", out.stderr);

    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("check-config touched the network: {other:?}"),
    }
}
