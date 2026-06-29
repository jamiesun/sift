use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::Parser;

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

#[derive(Debug, Default)]
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
    models: Vec<FileModelConfig>,
}

#[derive(Debug, Clone, Default)]
struct FileModelConfig {
    role: Option<Role>,
    endpoint: Option<String>,
    model: Option<String>,
    key_env: Option<String>,
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
    models: Vec<FileModelConfig>,
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
            models: file.models,
        })
    }

    /// 据降级寻址结果装配多模型注册表：[[model]] 优先，保留旧式 large/api_key + SIFT_SMALL_KEY 兜底。
    /// 缺 large 即降级（degraded=true），上层回退 AST-only，绝不阻断。
    pub fn build_registry(&self) -> Registry {
        let mut small = Vec::new();
        let mut large = None;

        for model in &self.models {
            let Some(client) = self.client_from_model_config(model) else {
                continue;
            };
            match client.role() {
                Role::Small => small.push(client),
                Role::Large if large.is_none() => large = Some(client),
                Role::Large => {}
            }
        }

        if large.is_none() {
            large = self.fallback_large();
        }
        if let Ok(k) = std::env::var(ENV_SMALL_KEY) {
            small.push(self.new_client(
                Role::Small,
                self.small_endpoint.clone(),
                self.small_model.clone(),
                k,
                self.timeout_ms,
                self.max_retries,
            ));
        }
        Registry { small, large }
    }

    fn client_from_model_config(&self, m: &FileModelConfig) -> Option<ModelClient> {
        let role = m.role?;
        let key_env = m.key_env.as_ref()?;
        let key = std::env::var(key_env).ok()?;
        let endpoint = m
            .endpoint
            .clone()
            .unwrap_or_else(|| self.default_endpoint_for(role));
        let model = m
            .model
            .clone()
            .unwrap_or_else(|| self.default_model_for(role));
        Some(self.new_client(
            role,
            endpoint,
            model,
            key,
            m.timeout_ms.unwrap_or(self.timeout_ms),
            m.max_retries.unwrap_or(self.max_retries),
        ))
    }

    fn fallback_large(&self) -> Option<ModelClient> {
        let key = self.api_key.clone()?;
        let large_cfg = self.models.iter().find(|m| m.role == Some(Role::Large));
        let endpoint = large_cfg
            .and_then(|m| m.endpoint.clone())
            .unwrap_or_else(|| self.endpoint.clone());
        let model = large_cfg
            .and_then(|m| m.model.clone())
            .unwrap_or_else(|| self.model.clone());
        let timeout_ms = large_cfg
            .and_then(|m| m.timeout_ms)
            .unwrap_or(self.timeout_ms);
        let max_retries = large_cfg
            .and_then(|m| m.max_retries)
            .unwrap_or(self.max_retries);
        Some(self.new_client(Role::Large, endpoint, model, key, timeout_ms, max_retries))
    }

    fn new_client(
        &self,
        role: Role,
        endpoint: String,
        model: String,
        key: String,
        timeout_ms: u64,
        max_retries: u32,
    ) -> ModelClient {
        let spec = ModelSpec {
            role,
            endpoint,
            model,
            key,
            timeout: std::time::Duration::from_millis(timeout_ms),
            max_retries,
        };
        ModelClient::new(spec, Box::new(UreqTransport), 3)
    }

    fn default_endpoint_for(&self, role: Role) -> String {
        match role {
            Role::Small => self.small_endpoint.clone(),
            Role::Large => self.endpoint.clone(),
        }
    }

    fn default_model_for(&self, role: Role) -> String {
        match role {
            Role::Small => self.small_model.clone(),
            Role::Large => self.model.clone(),
        }
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

/// 极简 key=value + [[model]] 解析，避免引入完整 toml 依赖；解析失败静默回退默认。
fn basic_toml_parse(src: &str) -> Option<FileConfig> {
    let mut cfg = FileConfig::default();
    let mut current_model: Option<FileModelConfig> = None;
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[model]]" {
            push_model_if_valid(&mut cfg, current_model.take());
            current_model = Some(FileModelConfig::default());
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"');
        if let Some(model) = current_model.as_mut() {
            parse_model_key(model, k.trim(), v);
        } else {
            parse_root_key(&mut cfg, k.trim(), v);
        }
    }
    push_model_if_valid(&mut cfg, current_model);
    Some(cfg)
}

fn parse_root_key(cfg: &mut FileConfig, key: &str, value: &str) {
    match key {
        "api_key" => cfg.api_key = Some(value.to_string()),
        "concurrency" => cfg.concurrency = value.parse().ok(),
        "max_bytes" => cfg.max_bytes = value.parse().ok(),
        "endpoint" => cfg.endpoint = Some(value.to_string()),
        "model" => cfg.model = Some(value.to_string()),
        "small_endpoint" => cfg.small_endpoint = Some(value.to_string()),
        "small_model" => cfg.small_model = Some(value.to_string()),
        "timeout_ms" => cfg.timeout_ms = value.parse().ok(),
        "max_retries" => cfg.max_retries = value.parse().ok(),
        _ => {}
    }
}

fn parse_model_key(model: &mut FileModelConfig, key: &str, value: &str) {
    match key {
        "role" => model.role = parse_role(value),
        "endpoint" => model.endpoint = Some(value.to_string()),
        "model" => model.model = Some(value.to_string()),
        "key_env" => model.key_env = Some(value.to_string()),
        "timeout_ms" => model.timeout_ms = value.parse().ok(),
        "max_retries" => model.max_retries = value.parse().ok(),
        _ => {}
    }
}

fn parse_role(value: &str) -> Option<Role> {
    match value.trim() {
        "small" => Some(Role::Small),
        "large" => Some(Role::Large),
        _ => None,
    }
}

fn push_model_if_valid(cfg: &mut FileConfig, model: Option<FileModelConfig>) {
    if let Some(model) = model
        && model.role.is_some()
    {
        cfg.models.push(model);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_keys_and_skips_comments() {
        let cfg =
            basic_toml_parse("# c\napi_key=\"k\"\nconcurrency=8\nbad line\n").unwrap_or_default();
        assert_eq!(cfg.api_key.as_deref(), Some("k"));
        assert_eq!(cfg.concurrency, Some(8));
    }

    #[test]
    fn dirty_values_fall_back_to_none_not_panic() {
        let cfg = basic_toml_parse("concurrency=oops\nmax_bytes=\n").unwrap_or_default();
        assert_eq!(cfg.concurrency, None);
        assert_eq!(cfg.max_bytes, None);
    }

    #[test]
    fn parses_model_blocks() {
        let cfg = basic_toml_parse(
            r#"
concurrency=2
[[model]]
role="small"
endpoint="https://small.example/v1"
model="cheap"
key_env="SIFT_SMALL_A"
timeout_ms=8000
[[model]]
role="large"
model="frontier"
key_env="SIFT_LARGE"
max_retries=2
"#,
        )
        .unwrap_or_default();
        assert_eq!(cfg.concurrency, Some(2));
        assert_eq!(cfg.models.len(), 2);
        assert_eq!(cfg.models[0].role, Some(Role::Small));
        assert_eq!(cfg.models[0].key_env.as_deref(), Some("SIFT_SMALL_A"));
        assert_eq!(cfg.models[0].timeout_ms, Some(8000));
        assert_eq!(cfg.models[1].role, Some(Role::Large));
        assert_eq!(cfg.models[1].model.as_deref(), Some("frontier"));
        assert_eq!(cfg.models[1].max_retries, Some(2));
    }

    #[test]
    fn ignores_model_blocks_without_role() {
        let cfg = basic_toml_parse("[[model]]\nmodel=\"x\"\n").unwrap_or_default();
        assert!(cfg.models.is_empty());
    }

    #[test]
    fn concurrency_never_zero() {
        assert!(default_concurrency() >= 1);
    }
}
