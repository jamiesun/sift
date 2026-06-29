use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::Parser;
use serde::Deserialize;

const ENV_API_KEY: &str = "SIFT_API_KEY";
const DEFAULT_IGNORES: &[&str] = &[
    "node_modules", "target", "dist", "build", "vendor", ".venv", "__pycache__",
];

/// sift — 可控成本的开源项目审计器。
#[derive(Parser, Debug)]
#[command(name = "sift", version, about = "分级漏斗审计：AST 脱水 → 小模型粗筛 → 大模型收敛")]
pub struct Cli {
    /// 要审计的项目根目录
    #[arg(default_value = ".")]
    pub target: PathBuf,

    /// 仅审计子模块路径（项目内相对/绝对皆可），扫描根重置到此
    #[arg(long)]
    pub module: Option<PathBuf>,

    /// 前沿模型 API Key（降级链最高优先级）
    #[arg(long)]
    pub api_key: Option<String>,

    /// 并发解析线程数
    #[arg(long)]
    pub concurrency: Option<usize>,

    /// 单文件字节上限，超过即跳过
    #[arg(long)]
    pub max_bytes: Option<u64>,

    /// 只跑扫描层，不连模型（无需 Key）
    #[arg(long)]
    pub scan_only: bool,
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    api_key: Option<String>,
    concurrency: Option<usize>,
    max_bytes: Option<u64>,
    ignores: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct Config {
    pub root: PathBuf,
    pub api_key: Option<String>,
    pub concurrency: usize,
    pub max_bytes: u64,
    pub ignores: Vec<String>,
    pub scan_only: bool,
}

impl Config {
    /// 降级寻址：CLI > ENV > config.toml；默认值兜底。
    pub fn resolve(cli: Cli) -> Result<Self> {
        let file = load_file_config();

        let root = match &cli.module {
            Some(m) if m.is_absolute() => m.clone(),
            Some(m) => cli.target.join(m),
            None => cli.target.clone(),
        };
        let root = root
            .canonicalize()
            .map_err(|e| anyhow!("无法定位审计根 {}: {e}", root.display()))?;

        let api_key = cli
            .api_key
            .or_else(|| std::env::var(ENV_API_KEY).ok())
            .or(file.api_key);

        let concurrency = cli
            .concurrency
            .or(file.concurrency)
            .unwrap_or_else(default_concurrency)
            .max(1);
        let max_bytes = cli.max_bytes.or(file.max_bytes).unwrap_or(512 * 1024);
        let ignores = file
            .ignores
            .unwrap_or_else(|| DEFAULT_IGNORES.iter().map(|s| s.to_string()).collect());

        Ok(Self { root, api_key, concurrency, max_bytes, ignores, scan_only: cli.scan_only })
    }
}

fn default_concurrency() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

fn config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/sift/config.toml"))
}

fn load_file_config() -> FileConfig {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| basic_toml_parse(&s))
        .unwrap_or_default()
}

/// 极简 key=value 解析，避免引入完整 toml 依赖；解析失败静默回退默认。
fn basic_toml_parse(src: &str) -> Option<FileConfig> {
    let mut cfg = FileConfig::default();
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim().trim_matches('"');
        match k.trim() {
            "api_key" => cfg.api_key = Some(v.to_string()),
            "concurrency" => cfg.concurrency = v.parse().ok(),
            "max_bytes" => cfg.max_bytes = v.parse().ok(),
            _ => {}
        }
    }
    Some(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_keys_and_skips_comments() {
        let cfg = basic_toml_parse("# c\napi_key=\"k\"\nconcurrency=8\nbad line\n").unwrap();
        assert_eq!(cfg.api_key.as_deref(), Some("k"));
        assert_eq!(cfg.concurrency, Some(8));
    }

    #[test]
    fn dirty_values_fall_back_to_none_not_panic() {
        let cfg = basic_toml_parse("concurrency=oops\nmax_bytes=\n").unwrap();
        assert_eq!(cfg.concurrency, None);
        assert_eq!(cfg.max_bytes, None);
    }

    #[test]
    fn concurrency_never_zero() {
        assert!(default_concurrency() >= 1);
    }
}
