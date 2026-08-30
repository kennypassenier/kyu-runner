// L0 · the test harness itself. A flaky harness is a broken gate
// (standing rule 7), so the mechanism that failed on 2026-08-30 —
// two test binaries being handed the same "free" port — has its own
// regression test.

mod support;

use support::*;

#[test]
fn l0_a_port_can_only_be_reserved_once() {
    // The cross-process claim: whoever creates the file wins, everyone
    // else must be told no. This is the exact property that was missing
    // when `cargo test --all` ran five suites side by side.
    let port = 65_123;
    let first = reserve(port);
    let second = reserve(port);
    assert!(
        !(first && second),
        "the same port was reserved twice — the claim is not atomic"
    );
    assert!(!reserve(port), "a taken port must stay taken");
}

#[test]
fn l0_free_ports_are_never_handed_out_twice() {
    let ports: Vec<u16> = (0..25).map(|_| free_port()).collect();
    let unique: std::collections::HashSet<u16> = ports.iter().copied().collect();
    assert_eq!(
        unique.len(),
        ports.len(),
        "free_port handed out a duplicate"
    );
}
