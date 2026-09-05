// L1 · W2 (--check, the kit's form of --check-config) + K8 (fail-closed startup) at the binary
// boundary: real process, real exit codes, remedies on stderr.

use std::io::Write;
use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    // KYU_RUNNER_BIN lets the release workflow run this suite against the
    // release artifact (T8/M1). The kit's --check probes the state
    // directory (rule 12), so every run gets a scratch one.
    let binary =
        std::env::var("KYU_RUNNER_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_kyu-runner").into());
    let state = tempfile::tempdir().expect("state dir");
    Command::new(binary)
        .args(args)
        .arg("--state-dir")
        .arg(state.path())
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
name = "kyu-events"
topic = "kyu.events"
subscription = "ha-runner"
webhook_url = "http://ha.lan:8123/api/webhook/hub_kyu_events"
"#;

#[test]
fn l1_w2_check_config_exits_zero_on_a_valid_config() {
    let file = config_file(VALID);
    let out = run(&["--config", file.path().to_str().unwrap(), "--check"]);
    assert!(out.status.success(), "stderr: {:?}", out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("config OK: 1 route(s)"), "{stdout}");
}

#[test]
fn l1_w2_check_config_fails_with_a_remedy_on_an_invalid_config() {
    let file = config_file(&VALID.replace("hub_url", "hub_uri"));
    let out = run(&["--config", file.path().to_str().unwrap(), "--check"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Remedy:"), "{stderr}");
}

#[test]
fn l1_k8_a_missing_config_file_fails_with_a_remedy() {
    let out = run(&["--config", "/nonexistent/kyu-runner.toml", "--check"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The kit reads the file and words its remedies "What now:"; the pump's
    // own config errors keep "Remedy:". Either way the operator gets one.
    assert!(
        stderr.contains("What now:") || stderr.contains("Remedy:"),
        "{stderr}"
    );
}

#[test]
fn l1_w2_an_unknown_argument_fails_with_a_remedy() {
    let out = run(&["--frobnicate"]);
    // The kit's convention: every refusal exits 1 with a remedy.
    assert_eq!(out.status.code(), Some(1));
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
    let out = run(&["--config", file.path().to_str().unwrap(), "--check"]);
    assert!(out.status.success(), "stderr: {:?}", out.stderr);

    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("check-config touched the network: {other:?}"),
    }
}
