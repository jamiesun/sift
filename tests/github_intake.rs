use std::process::Command;

#[test]
fn github_intake_rejects_non_github_url_without_network() {
    let output = Command::new(env!("CARGO_BIN_EXE_sift"))
        .args([
            "github",
            "https://example.com/owner/repo",
            "--agent-gate",
            "--no-build",
            "--no-install",
        ])
        .output()
        .expect("run sift github");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.contains("only https://github.com/owner/repo or owner/repo are supported"));
}
