use assert_cmd::Command;

#[test]
fn root_help_lists_sessions_subcommand() {
    let output = Command::cargo_bin("request-pilot")
        .unwrap()
        .arg("--help")
        .output()
        .expect("failed to run --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("sessions"),
        "expected `sessions` in --help output, got:\n{}",
        stdout
    );
}

#[test]
fn sessions_help_lists_subcommands() {
    let output = Command::cargo_bin("request-pilot")
        .unwrap()
        .args(["sessions", "--help"])
        .output()
        .expect("failed to run sessions --help");
    assert!(output.status.success(), "sessions --help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for sub in ["list", "show", "replay", "export", "stats", "prune"] {
        assert!(
            stdout.contains(sub),
            "expected `{}` in sessions --help output, got:\n{}",
            sub,
            stdout
        );
    }
}
