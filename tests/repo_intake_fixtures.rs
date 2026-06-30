use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct FixtureCase {
    name: &'static str,
    verdict: &'static str,
    rules: &'static [&'static str],
}

#[test]
fn malicious_repo_intake_fixtures_trip_expected_rules() {
    let cases = [
        FixtureCase {
            name: "npm-postinstall-download",
            verdict: "REJECT",
            rules: &["npm-lifecycle-script", "download-execute"],
        },
        FixtureCase {
            name: "python-setup-command",
            verdict: "REJECT",
            rules: &["python-setup-command"],
        },
        FixtureCase {
            name: "rust-build-command",
            verdict: "REJECT",
            rules: &["rust-build-script-command"],
        },
        FixtureCase {
            name: "docker-curl-pipe",
            verdict: "REJECT",
            rules: &["download-execute"],
        },
        FixtureCase {
            name: "makefile-hidden-network",
            verdict: "REJECT",
            rules: &["download-execute"],
        },
        FixtureCase {
            name: "github-action-secret-shell",
            verdict: "CAUTION",
            rules: &["workflow-secret-shell", "unpinned-github-action"],
        },
        FixtureCase {
            name: "shell-home-write",
            verdict: "REJECT",
            rules: &["install-home-write"],
        },
        FixtureCase {
            name: "base64-shell",
            verdict: "REJECT",
            rules: &["base64-execute"],
        },
        FixtureCase {
            name: "binary-artifact-exec",
            verdict: "REJECT",
            rules: &["download-execute"],
        },
        FixtureCase {
            name: "readme-dangerous-install",
            verdict: "REJECT",
            rules: &["download-execute"],
        },
    ];

    for case in cases {
        let output = run_agent_gate(case.name);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            !output.status.success(),
            "fixture {} should block agent execution\nstdout:\n{}\nstderr:\n{}",
            case.name,
            stdout,
            stderr
        );
        assert!(
            stdout.contains(&format!("VERDICT: {}", case.verdict)),
            "fixture {} expected verdict {}\nstdout:\n{}",
            case.name,
            case.verdict,
            stdout
        );
        assert!(
            stdout.contains("SAFE_TO_AGENT_RUN: no"),
            "fixture {} should not be safe to run\nstdout:\n{}",
            case.name,
            stdout
        );
        for rule in case.rules {
            assert!(
                stdout.contains(rule),
                "fixture {} missing expected rule {}\nstdout:\n{}",
                case.name,
                rule,
                stdout
            );
        }
    }
}

#[test]
fn benign_repo_intake_fixture_is_accepted() {
    let output = run_agent_gate("benign-controls");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "benign fixture should pass\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    assert!(stdout.contains("VERDICT: ACCEPT"));
    assert!(stdout.contains("SAFE_TO_AGENT_RUN: yes"));
}

fn run_agent_gate(name: &str) -> Output {
    let home = unique_home(name);
    fs::create_dir_all(&home).expect("create isolated HOME");
    let fixture = fixture_root().join(name);
    let output = Command::new(env!("CARGO_BIN_EXE_sift"))
        .arg(&fixture)
        .arg("--agent-gate")
        .env("HOME", &home)
        .env_remove("SIFT_INTERNAL_GATE")
        .env_remove("SIFT_API_KEY")
        .env_remove("SIFT_SMALL_KEY")
        .output()
        .expect("run sift agent gate");
    fs::remove_dir_all(&home).ok();
    output
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo-intake")
}

fn unique_home(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "sift-fixture-home-{name}-{}-{nanos}",
        std::process::id()
    ))
}
