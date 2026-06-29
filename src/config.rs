use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::Parser;
use serde::Deserialize;

use crate::model::{ModelClient, ModelSpec, Registry, Role, UreqTransport};

const ENV_API_KEY: &str = "SIFT_API_KEY";
const ENV_SMALL_KEY: &str = "SIFT_SMALL_KEY";
const DEFAULT_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";
const DEFAULT_LARGE_MODEL: &str = "gpt-4o";
const DEFAULT_SMALL_MODEL: &str = "gpt-4o-mini";
const DEFAULT_IGNORES: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    ".venv",
    "__pycache__",
];

/// sift — 可控成本的开源项目审计器。
#[derive(Parser, Debug)]
#[command(
    name = "sift",
    version,
    about = "分级漏斗审计：AST 脱水 → 小模型粗筛 → 大模型收敛"
)]
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
    endpoint: Option<String>,
    model: Option<String>,
    small_endpoint: Option<String>,
    small_model: Option<String>,
    timeout_ms: Option<u64>,
    max_retries: Option<u32>,
}

#[derive(Debug)]
pub struct Config {
    pub root: PathBuf,
    pub api_key: Option<String>,
    pub concurrency: usize,
    pub max_bytes: u64,
    pub ignores: Vec<String>,
    pub scan_only: bool,
    pub endpoint: String,
    pub model: String,
    pub small_endpoint: String,
    pub small_model: String,
    pub timeout_ms: u64,
    pub max_retries: u32,
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

        Ok(Self {
            root,
            api_key,
            concurrency,
            max_bytes,
            ignores,
            scan_only: cli.scan_only,
            endpoint: file
                .endpoint
                .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
            model: file
                .model
                .unwrap_or_else(|| DEFAULT_LARGE_MODEL.to_string()),
            small_endpoint: file
                .small_endpoint
                .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
            small_model: file
                .small_model
                .unwrap_or_else(|| DEFAULT_SMALL_MODEL.to_string()),
            timeout_ms: file.timeout_ms.unwrap_or(60_000),
            max_retries: file.max_retries.unwrap_or(1),
        })
    }

    /// 据降级寻址结果装配多模型注册表：large=本机 api_key，small=SIFT_SMALL_KEY。
    /// 缺 large 即降级（degraded=true），上层回退 AST-only，绝不阻断。
    pub fn build_registry(&self) -> Registry {
        let timeout = std::time::Duration::from_millis(self.timeout_ms);
        let large = self.api_key.as_ref().map(|k| {
            let spec = ModelSpec {
                role: Role::Large,
                endpoint: self.endpoint.clone(),
                model: self.model.clone(),
                key: k.clone(),
                timeout,
                max_retries: self.max_retries,
            };
            ModelClient::new(spec, Box::new(UreqTransport), 3)
        });
        let mut small = Vec::new();
        if let Ok(k) = std::env::var(ENV_SMALL_KEY) {
            let spec = ModelSpec {
                role: Role::Small,
                endpoint: self.small_endpoint.clone(),
                model: self.small_model.clone(),
                key: k,
                timeout,
                max_retries: self.max_retries,
            };
            small.push(ModelClient::new(spec, Box::new(UreqTransport), 3));
        }
        Registry { small, large }
    }
}

fn default_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
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
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"');
        match k.trim() {
            "api_key" => cfg.api_key = Some(v.to_string()),
            "concurrency" => cfg.concurrency = v.parse().ok(),
            "max_bytes" => cfg.max_bytes = v.parse().ok(),
            "endpoint" => cfg.endpoint = Some(v.to_string()),
            "model" => cfg.model = Some(v.to_string()),
            "small_endpoint" => cfg.small_endpoint = Some(v.to_string()),
            "small_model" => cfg.small_model = Some(v.to_string()),
            "timeout_ms" => cfg.timeout_ms = v.parse().ok(),
            "max_retries" => cfg.max_retries = v.parse().ok(),
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
