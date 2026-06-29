use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::Parser;

use crate::model::{ModelClient, ModelSpec, Registry, Role, UreqTransport};

const ENV_API_KEY: &str = "SIFT_API_KEY";
const ENV_SMALL_KEY: &str = "SIFT_SMALL_KEY";
const ENV_ENDPOINT: &str = "SIFT_ENDPOINT";
const ENV_MODEL: &str = "SIFT_MODEL";
const ENV_SMALL_ENDPOINT: &str = "SIFT_SMALL_ENDPOINT";
const ENV_SMALL_MODEL: &str = "SIFT_SMALL_MODEL";
const ENV_TIMEOUT_MS: &str = "SIFT_TIMEOUT_MS";
const ENV_MAX_RETRIES: &str = "SIFT_MAX_RETRIES";
const ENV_REF_SUFFIX: &str = "_ENV";
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

/// sift: a cost-controlled open-source project auditor.
#[derive(Parser, Debug)]
#[command(
    name = "sift",
    version,
    about = "Tiered audit: AST dehydration -> small-model Map -> large-model Reduce"
)]
pub struct Cli {
    /// Project root to audit
    #[arg(default_value = ".")]
    pub target: PathBuf,

    /// Audit only this submodule path inside the project root
    #[arg(long)]
    pub module: Option<PathBuf>,

    /// Read the large-model API key from a file
    #[arg(long)]
    pub api_key_file: Option<PathBuf>,

    /// Parser concurrency
    #[arg(long)]
    pub concurrency: Option<usize>,

    /// Per-file byte limit; larger files are skipped
    #[arg(long)]
    pub max_bytes: Option<u64>,

    /// Run only scan/dehydrate and do not call models
    #[arg(long)]
    pub scan_only: bool,

    /// Run local self-audit and write reports/self-audit.md
    #[arg(long)]
    pub self_audit: bool,
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
    pub project_root: PathBuf,
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
    pub self_audit: bool,
    models: Vec<FileModelConfig>,
    env_file: BTreeMap<String, String>,
}

impl Config {
    /// Resolve order: CLI key file > ENV > project .env > config.toml; defaults fill non-secret fields.
    pub fn resolve(cli: Cli) -> Result<Self> {
        let project_root = cli
            .target
            .canonicalize()
            .map_err(|e| anyhow!("cannot locate project root {}: {e}", cli.target.display()))?;
        let root_candidate = match &cli.module {
            Some(m) if m.is_absolute() => m.clone(),
            Some(m) => project_root.join(m),
            None => cli.target.clone(),
        };
        let root = root_candidate
            .canonicalize()
            .map_err(|e| anyhow!("cannot locate audit root {}: {e}", root_candidate.display()))?;
        if cli.module.is_some() && !root.starts_with(&project_root) {
            return Err(anyhow!(
                "module path {} is outside project root {}",
                root.display(),
                project_root.display()
            ));
        }
        let file = load_file_config()?;
        let env_file = load_env_file(&project_root.join(".env"))?;

        let api_key = cli
            .api_key_file
            .as_deref()
            .and_then(read_key_file)
            .or_else(|| env_key_value(&env_file, ENV_API_KEY))
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
            project_root,
            root,
            api_key,
            concurrency,
            max_bytes,
            ignores,
            scan_only: cli.scan_only,
            endpoint: env_value(&env_file, ENV_ENDPOINT)
                .or(file.endpoint)
                .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
            model: env_value(&env_file, ENV_MODEL)
                .or(file.model)
                .unwrap_or_else(|| DEFAULT_LARGE_MODEL.to_string()),
            small_endpoint: env_value(&env_file, ENV_SMALL_ENDPOINT)
                .or(file.small_endpoint)
                .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
            small_model: env_value(&env_file, ENV_SMALL_MODEL)
                .or(file.small_model)
                .unwrap_or_else(|| DEFAULT_SMALL_MODEL.to_string()),
            timeout_ms: env_u64(&env_file, ENV_TIMEOUT_MS)
                .or(file.timeout_ms)
                .unwrap_or(60_000),
            max_retries: env_u64(&env_file, ENV_MAX_RETRIES)
                .and_then(|v| u32::try_from(v).ok())
                .or(file.max_retries)
                .unwrap_or(1),
            self_audit: cli.self_audit,
            models: file.models,
            env_file,
        })
    }

    /// Build the model registry from [[model]] blocks first, then legacy fallbacks.
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
        if let Some(k) = self.lookup_env(ENV_SMALL_KEY) {
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
        let key = self.lookup_env(key_env)?;
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

    fn lookup_env(&self, key: &str) -> Option<String> {
        env_key_value(&self.env_file, key)
    }
}

pub fn missing_large_key_hint() -> &'static str {
    "missing large-model API key. Provide one of:\n  export SIFT_API_KEY=<KEY>\n  .env:\n    SIFT_API_KEY_ENV=WJT_AZURE_OPENAI_API_KEY\n  sift ./repo --api-key-file ~/.config/sift/key\n  ~/.config/sift/config.toml:\n    api_key = \"<KEY>\"\n  ~/.config/sift/config.toml:\n    [[model]]\n    role = \"large\"\n    key_env = \"SIFT_API_KEY\"\nOr use --scan-only for scan/dehydrate only, or --self-audit for local gates."
}

fn default_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

fn config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/sift/config.toml"))
}

fn load_file_config() -> Result<FileConfig> {
    let Some(path) = config_path() else {
        return Ok(FileConfig::default());
    };
    let src = match std::fs::read_to_string(&path) {
        Ok(src) => src,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(FileConfig::default()),
        Err(e) => return Err(anyhow!("cannot read config file {}: {e}", path.display())),
    };
    parse_file_config(&src).with_context(|| format!("invalid config file {}", path.display()))
}

fn load_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let src = match std::fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(anyhow!("cannot read env file {}: {e}", path.display())),
    };
    parse_env_file(&src).with_context(|| format!("invalid env file {}", path.display()))
}

fn parse_env_file(src: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (idx, raw) in src.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = line.split_once('=') else {
            return Err(anyhow!("line {} is missing '='", idx + 1));
        };
        let key = key.trim();
        if !valid_env_key(key) {
            return Err(anyhow!("line {} has invalid key", idx + 1));
        }
        out.insert(key.to_string(), parse_env_value(value.trim())?);
    }
    Ok(out)
}

fn valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn parse_env_value(value: &str) -> Result<String> {
    if let Some(stripped) = value.strip_prefix('"') {
        let Some(end) = stripped.rfind('"') else {
            return Err(anyhow!("unterminated double-quoted env value"));
        };
        return Ok(stripped[..end].to_string());
    }
    if let Some(stripped) = value.strip_prefix('\'') {
        let Some(end) = stripped.rfind('\'') else {
            return Err(anyhow!("unterminated single-quoted env value"));
        };
        return Ok(stripped[..end].to_string());
    }
    Ok(value
        .split_once(" #")
        .map(|(v, _)| v)
        .unwrap_or(value)
        .trim()
        .to_string())
}

fn env_value(env_file: &BTreeMap<String, String>, key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .or_else(|| env_file.get(key).cloned())
}

fn env_key_value(env_file: &BTreeMap<String, String>, key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .or_else(|| {
            let ref_key = format!("{key}{ENV_REF_SUFFIX}");
            env_file
                .get(&ref_key)
                .and_then(|source_key| std::env::var(source_key).ok())
        })
        .or_else(|| {
            if key == ENV_API_KEY {
                None
            } else {
                env_file.get(key).cloned()
            }
        })
}

fn env_u64(env_file: &BTreeMap<String, String>, key: &str) -> Option<u64> {
    env_value(env_file, key).and_then(|v| v.parse().ok())
}

fn read_key_file(path: &Path) -> Option<String> {
    let key = std::fs::read_to_string(path).ok()?;
    let key = key.trim();
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
}

fn parse_file_config(src: &str) -> Result<FileConfig> {
    let parsed: toml::Value = toml::from_str(src).context("parse TOML")?;
    let table = parsed
        .as_table()
        .ok_or_else(|| anyhow!("config root must be a TOML table"))?;
    let mut cfg = FileConfig {
        api_key: table_string(table, "api_key"),
        concurrency: table_u64(table, "concurrency").and_then(|v| usize::try_from(v).ok()),
        max_bytes: table_u64(table, "max_bytes"),
        ignores: table_string_array(table, "ignores"),
        endpoint: table_string(table, "endpoint"),
        model: table_string(table, "model"),
        small_endpoint: table_string(table, "small_endpoint"),
        small_model: table_string(table, "small_model"),
        timeout_ms: table_u64(table, "timeout_ms"),
        max_retries: table_u64(table, "max_retries").and_then(|v| u32::try_from(v).ok()),
        models: Vec::new(),
    };

    if let Some(models) = table.get("model").and_then(|v| v.as_array()) {
        for item in models {
            let Some(t) = item.as_table() else {
                continue;
            };
            let model = FileModelConfig {
                role: table_string(t, "role").and_then(|r| parse_role(&r)),
                endpoint: table_string(t, "endpoint"),
                model: table_string(t, "model"),
                key_env: table_string(t, "key_env"),
                timeout_ms: table_u64(t, "timeout_ms"),
                max_retries: table_u64(t, "max_retries").and_then(|v| u32::try_from(v).ok()),
            };
            if model.role.is_some() {
                cfg.models.push(model);
            }
        }
    }

    Ok(cfg)
}

fn table_string(table: &toml::Table, key: &str) -> Option<String> {
    table.get(key)?.as_str().map(str::to_string)
}

fn table_u64(table: &toml::Table, key: &str) -> Option<u64> {
    let value = table.get(key)?;
    if let Some(i) = value.as_integer() {
        u64::try_from(i).ok()
    } else {
        None
    }
}

fn table_string_array(table: &toml::Table, key: &str) -> Option<Vec<String>> {
    let values = table.get(key)?.as_array()?;
    let out: Vec<String> = values
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    if out.len() == values.len() {
        Some(out)
    } else {
        None
    }
}

fn parse_role(value: &str) -> Option<Role> {
    match value.trim() {
        "small" => Some(Role::Small),
        "large" => Some(Role::Large),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_keys_and_skips_comments() {
        let cfg = parse_file_config("# c\napi_key=\"k\"\nconcurrency=8\n").unwrap_or_default();
        assert_eq!(cfg.api_key.as_deref(), Some("k"));
        assert_eq!(cfg.concurrency, Some(8));
    }

    #[test]
    fn dirty_values_reject_config_not_silent_default() {
        let err = parse_file_config("concurrency=oops\nmax_bytes=\n");
        assert!(err.is_err());
    }

    #[test]
    fn parses_model_blocks() {
        let cfg = parse_file_config(
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
        let cfg = parse_file_config("[[model]]\nmodel=\"x\"\n").unwrap_or_default();
        assert!(cfg.models.is_empty());
    }

    #[test]
    fn concurrency_never_zero() {
        assert!(default_concurrency() >= 1);
    }

    #[test]
    fn missing_key_hint_uses_parseable_model_block() {
        let hint = missing_large_key_hint();
        assert!(hint.contains("[[model]]\n    role = \"large\""));
        let snippet = r#"
[[model]]
role = "large"
key_env = "SIFT_API_KEY"
"#;
        let cfg = parse_file_config(snippet).unwrap_or_default();
        assert_eq!(cfg.models.len(), 1);
        assert_eq!(cfg.models[0].role, Some(Role::Large));
    }

    #[test]
    fn parses_documented_model_config() {
        let cfg = parse_file_config(
            r#"
concurrency = 8
[[model]]
role = "small"
endpoint = "https://small.example/v1"
key_env = "SIFT_SMALL_KEY"
timeout_ms = 8000
max_retries = 1
[[model]]
role = "large"
endpoint = "https://large.example/v1"
key_env = "SIFT_API_KEY"
timeout_ms = 60000
max_retries = 1
"#,
        )
        .unwrap_or_default();

        assert_eq!(cfg.concurrency, Some(8));
        assert_eq!(cfg.models.len(), 2);
        assert_eq!(
            cfg.models[0].endpoint.as_deref(),
            Some("https://small.example/v1")
        );
        assert_eq!(cfg.models[0].key_env.as_deref(), Some("SIFT_SMALL_KEY"));
        assert_eq!(cfg.models[0].max_retries, Some(1));
        assert_eq!(cfg.models[1].key_env.as_deref(), Some("SIFT_API_KEY"));
    }

    #[test]
    fn absolute_module_must_stay_inside_target() {
        let root = unique_test_dir("module-root");
        let outside = unique_test_dir("module-outside");
        std::fs::create_dir_all(root.join("src")).ok();
        std::fs::create_dir_all(&outside).ok();
        let cli = Cli {
            target: root.clone(),
            module: Some(outside),
            api_key_file: None,
            concurrency: None,
            max_bytes: None,
            scan_only: true,
            self_audit: false,
        };
        let err = Config::resolve(cli).err().map(|e| e.to_string());
        assert!(err.unwrap_or_default().contains("outside project root"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn absolute_module_inside_target_is_allowed() {
        let root = unique_test_dir("module-inside");
        let module = root.join("src");
        std::fs::create_dir_all(&module).ok();
        let cli = Cli {
            target: root.clone(),
            module: Some(module.clone()),
            api_key_file: None,
            concurrency: None,
            max_bytes: None,
            scan_only: true,
            self_audit: false,
        };
        let cfg = Config::resolve(cli).unwrap_or_else(|_| Config {
            project_root: PathBuf::new(),
            root: PathBuf::new(),
            api_key: None,
            concurrency: 1,
            max_bytes: 1,
            ignores: Vec::new(),
            scan_only: true,
            endpoint: String::new(),
            model: String::new(),
            small_endpoint: String::new(),
            small_model: String::new(),
            timeout_ms: 1,
            max_retries: 0,
            self_audit: false,
            models: Vec::new(),
            env_file: BTreeMap::new(),
        });
        assert_eq!(cfg.root, module.canonicalize().unwrap_or(module));
        std::fs::remove_dir_all(root).ok();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sift-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[test]
    fn parses_project_env_file() {
        let env = parse_env_file(
            r#"
# local
SIFT_API_KEY=large
SIFT_API_KEY_ENV=WJT_AZURE_OPENAI_API_KEY
export SIFT_SMALL_KEY='small'
SIFT_ENDPOINT="https://example.test/v1/chat/completions"
SIFT_MAX_RETRIES=2 # retry once after first attempt
"#,
        )
        .unwrap_or_default();

        assert_eq!(env.get("SIFT_API_KEY").map(String::as_str), Some("large"));
        assert_eq!(
            env.get("SIFT_API_KEY_ENV").map(String::as_str),
            Some("WJT_AZURE_OPENAI_API_KEY")
        );
        assert_eq!(env.get("SIFT_SMALL_KEY").map(String::as_str), Some("small"));
        assert_eq!(
            env.get("SIFT_ENDPOINT").map(String::as_str),
            Some("https://example.test/v1/chat/completions")
        );
        assert_eq!(env_u64(&env, "SIFT_MAX_RETRIES"), Some(2));
    }

    #[test]
    fn rejects_dirty_env_lines() {
        assert!(parse_env_file("bad line\n").is_err());
        assert!(parse_env_file("1BAD=x\n").is_err());
    }
}
