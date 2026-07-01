use std::path::PathBuf;
use std::thread;

use crossbeam_channel::{Receiver, bounded};
use ignore::WalkBuilder;

use crate::config::Config;

/// Bounded channel capacity; fast disk I/O back-pressures instead of growing memory.
const CHANNEL_CAP: usize = 1024;
const VCS_METADATA_DIRS: &[&str] = &[".git", ".hg", ".svn", ".jj"];

/// Start a background walk. The main loop consumes paths and drops them immediately.
pub fn spawn_scan(cfg: &Config) -> Receiver<PathBuf> {
    let (tx, rx) = bounded::<PathBuf>(CHANNEL_CAP);

    let root = cfg.root.clone();
    let ignores = cfg.ignores.clone();

    thread::spawn(move || {
        let mut builder = WalkBuilder::new(&root);
        builder.standard_filters(true).hidden(false);
        builder.filter_entry(move |e| {
            e.file_name()
                .to_str()
                .map(|name| !is_ignored_entry(name, &ignores))
                .unwrap_or(true)
        });

        for dent in builder.build() {
            let Ok(entry) = dent else { continue };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            if tx.send(entry.into_path()).is_err() {
                break;
            }
        }
    });

    rx
}

fn is_ignored_entry(name: &str, ignores: &[String]) -> bool {
    VCS_METADATA_DIRS.contains(&name) || ignores.iter().any(|ignore| ignore == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Cli, Config};

    #[test]
    fn scan_skips_ignored_dirs_and_large_files() {
        let root = unique_test_dir("scan");
        let ignored = root.join("target");
        let vcs = root.join(".git/hooks");
        let hidden_project = root.join(".github/workflows");
        assert!(std::fs::create_dir_all(&ignored).is_ok());
        assert!(std::fs::create_dir_all(&vcs).is_ok());
        assert!(std::fs::create_dir_all(&hidden_project).is_ok());
        assert!(std::fs::write(root.join("a.rs"), "fn a() {}").is_ok());
        assert!(std::fs::write(ignored.join("b.rs"), "fn b() {}").is_ok());
        assert!(std::fs::write(vcs.join("README.sample"), "hook notes").is_ok());
        assert!(std::fs::write(hidden_project.join("ci.yml"), "name: ci").is_ok());
        let cli = Cli {
            command: None,
            target: root.clone(),
            module: None,
            api_key_file: None,
            concurrency: None,
            max_bytes: Some(128),
            scan_only: true,
            agent_gate: false,
            format: crate::config::OutputFormat::Text,
            benchmark: false,
            benchmark_output: None,
            benchmark_input_1m_cost: None,
            benchmark_output_1m_cost: None,
            benchmark_estimated_output_tokens: None,
            report_language: crate::config::ReportLanguage::En,
            debug: false,
            save: false,
            save_to: None,
        };
        let cfg = Config::resolve(cli);
        assert!(cfg.is_ok(), "test config should resolve");
        let Ok(cfg) = cfg else {
            std::fs::remove_dir_all(root).ok();
            return;
        };
        let paths: Vec<PathBuf> = spawn_scan(&cfg).iter().collect();
        assert!(paths.iter().any(|p| p.ends_with("a.rs")));
        assert!(!paths.iter().any(|p| p.ends_with("b.rs")));
        assert!(!paths.iter().any(|p| p.ends_with("README.sample")));
        assert!(paths.iter().any(|p| p.ends_with("ci.yml")));
        std::fs::remove_dir_all(root).ok();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sift-scanner-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ))
    }
}
