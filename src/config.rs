use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand, ValueEnum};

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
const DEFAULT_CONFIG: &str = r#"# sift default configuration.
# This file is created automatically on first run at ~/.sift/config.toml.
# Keep secrets in environment variables or key files; do not paste API keys here.

max_bytes = 524288
endpoint = "https://api.openai.com/v1/chat/completions"
model = "gpt-4o"
small_endpoint = "https://api.openai.com/v1/chat/completions"
small_model = "gpt-4o-mini"
timeout_ms = 60000
max_retries = 1
ignores = ["node_modules", "target", "dist", "build", "vendor", ".venv", "__pycache__"]

# Example multi-model configuration:
# [[model]]
# role = "small"
# endpoint = "http://127.0.0.1:11434/v1/chat/completions"
# model = "gpt-oss:20b-cloud"
# key_env = "SIFT_SMALL_KEY"
# timeout_ms = 8000
# max_retries = 1
#
# [[model]]
# role = "large"
# endpoint = "https://api.openai.com/v1/chat/completions"
# model = "gpt-4o"
# key_env = "SIFT_API_KEY"
# timeout_ms = 60000
# max_retries = 1
"#;
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
    #[command(subcommand)]
    pub command: Option<CliCommand>,

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

    /// Run deterministic pre-run agent gate and do not call models
    #[arg(long)]
    pub agent_gate: bool,

    /// Output format for --agent-gate
    #[arg(long, value_enum, default_value = "text")]
    pub format: OutputFormat,

    /// Run scan/dehydrate benchmark telemetry and do not call models
    #[arg(long)]
    pub benchmark: bool,

    /// Write benchmark JSON to this path instead of stdout
    #[arg(long)]
    pub benchmark_output: Option<PathBuf>,

    /// Estimated input-token price per 1M tokens, in USD
    #[arg(long)]
    pub benchmark_input_1m_cost: Option<f64>,

    /// Estimated output-token price per 1M tokens, in USD
    #[arg(long)]
    pub benchmark_output_1m_cost: Option<f64>,

    /// Estimated output tokens to include in benchmark cost math
    #[arg(long)]
    pub benchmark_estimated_output_tokens: Option<u64>,

    /// Markdown report language
    #[arg(long, alias = "report-lang", value_enum, default_value = "en")]
    pub report_language: ReportLanguage,

    /// Save the audit result to reports/sift-audit-result-yyyymmdd-num.md
    #[arg(long)]
    pub save: bool,

    /// Directory to save the audit result into (implies --save)
    #[arg(long)]
    pub save_to: Option<PathBuf>,

    /// Print extra diagnostic progress to stderr
    #[arg(long)]
    pub debug: bool,
}

#[derive(Debug, Subcommand)]
pub enum CliCommand {
    /// Safely inspect a GitHub repository before local setup or agent execution
    Github(GithubCli),
    /// Run the checked-in repo-intake evaluation corpus
    EvalCorpus(EvalCorpusCli),
}

#[derive(Debug, Args)]
pub struct GithubCli {
    /// GitHub repository as owner/repo or https://github.com/owner/repo
    pub repo: String,

    /// Branch, tag, or commit SHA to fetch
    #[arg(long = "ref")]
    pub ref_name: Option<String>,

    /// Audit only this submodule path inside the fetched repository
    #[arg(long)]
    pub module: Option<PathBuf>,

    /// Keep the temporary checkout and print its path
    #[arg(long)]
    pub keep_checkout: bool,

    /// Explicit safety marker; repository code is never built
    #[arg(long)]
    pub no_build: bool,

    /// Explicit safety marker; repository code is never installed
    #[arg(long)]
    pub no_install: bool,

    /// Run only scan/dehydrate and do not call models
    #[arg(long)]
    pub scan_only: bool,

    /// Run deterministic pre-run agent gate and do not call models
    #[arg(long)]
    pub agent_gate: bool,

    /// Output format for --agent-gate
    #[arg(long, value_enum, default_value = "text")]
    pub format: OutputFormat,

    /// Maximum files allowed in the fetched checkout before scanning
    #[arg(long, default_value_t = 20_000)]
    pub max_checkout_files: usize,

    /// Maximum bytes allowed in the fetched checkout before scanning
    #[arg(long, default_value_t = 250 * 1024 * 1024)]
    pub max_checkout_bytes: u64,

    /// Run scan/dehydrate benchmark telemetry and do not call models
    #[arg(long)]
    pub benchmark: bool,

    /// Write benchmark JSON to this path instead of stdout
    #[arg(long)]
    pub benchmark_output: Option<PathBuf>,

    /// Estimated input-token price per 1M tokens, in USD
    #[arg(long)]
    pub benchmark_input_1m_cost: Option<f64>,

    /// Estimated output-token price per 1M tokens, in USD
    #[arg(long)]
    pub benchmark_output_1m_cost: Option<f64>,

    /// Estimated output tokens to include in benchmark cost math
    #[arg(long)]
    pub benchmark_estimated_output_tokens: Option<u64>,

    /// Markdown report language
    #[arg(long, alias = "report-lang", value_enum, default_value = "en")]
    pub report_language: ReportLanguage,

    /// Print extra diagnostic progress to stderr
    #[arg(long)]
    pub debug: bool,
}

#[derive(Debug, Args)]
pub struct EvalCorpusCli {
    /// Fixture corpus root; defaults to tests/fixtures/repo-intake
    #[arg(long)]
    pub fixtures: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ReportLanguage {
    En,
    Zh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

impl ReportLanguage {
    pub fn code(self) -> &'static str {
        match self {
            ReportLanguage::En => "en",
            ReportLanguage::Zh => "zh",
        }
    }

    pub fn prompt_instruction(self) -> &'static str {
        match self {
            ReportLanguage::En => "Write the final Markdown report in English.",
            ReportLanguage::Zh => "Write the final Markdown report in Simplified Chinese.",
        }
    }
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
    pub agent_gate: bool,
    pub format: OutputFormat,
    pub benchmark: bool,
    pub benchmark_output: Option<PathBuf>,
    pub benchmark_input_1m_cost: Option<f64>,
    pub benchmark_output_1m_cost: Option<f64>,
    pub benchmark_estimated_output_tokens: u64,
    pub endpoint: String,
    pub model: String,
    pub small_endpoint: String,
    pub small_model: String,
    pub timeout_ms: u64,
    pub max_retries: u32,
    pub report_language: ReportLanguage,
    pub debug: bool,
    pub save: bool,
    pub save_to: Option<PathBuf>,
    pub policy: Policy,
    models: Vec<FileModelConfig>,
    env_file: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub max_candidate_files: Option<usize>,
    pub allowlist: Vec<PolicyMatcher>,
    pub denylist: Vec<PolicyMatcher>,
    pub severity_overrides: Vec<PolicySeverityOverride>,
}

#[derive(Debug, Clone, Default)]
pub struct PolicyMatcher {
    pub path: Option<String>,
    pub rule: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PolicySeverityOverride {
    pub path: Option<String>,
    pub rule: Option<String>,
    pub severity: String,
    pub reason: Option<String>,
}

impl Config {
    /// Resolve order: CLI key file > ENV > project .env > ~/.sift/config.toml; defaults fill non-secret fields.
    pub fn resolve(cli: Cli) -> Result<Self> {
        let exclusive_modes = [cli.scan_only, cli.agent_gate, cli.benchmark]
            .iter()
            .filter(|enabled| **enabled)
            .count();
        if exclusive_modes > 1 {
            return Err(anyhow!(
                "--scan-only, --agent-gate, and --benchmark cannot be combined"
            ));
        }
        validate_price("benchmark-input-1m-cost", cli.benchmark_input_1m_cost)?;
        validate_price("benchmark-output-1m-cost", cli.benchmark_output_1m_cost)?;

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

        let api_key = match cli.api_key_file.as_deref() {
            Some(path) => Some(read_key_file(path)?),
            None => env_key_value(&env_file, ENV_API_KEY).or(file.api_key),
        };

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
            agent_gate: cli.agent_gate,
            format: cli.format,
            benchmark: cli.benchmark,
            benchmark_output: cli.benchmark_output,
            benchmark_input_1m_cost: cli.benchmark_input_1m_cost,
            benchmark_output_1m_cost: cli.benchmark_output_1m_cost,
            benchmark_estimated_output_tokens: cli.benchmark_estimated_output_tokens.unwrap_or(0),
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
            report_language: cli.report_language,
            debug: cli.debug,
            save: cli.save || cli.save_to.is_some(),
            save_to: cli.save_to,
            policy: load_policy_config(&project_root.join("sift-policy.toml"))?,
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
        let endpoint = m
            .endpoint
            .clone()
            .unwrap_or_else(|| self.default_endpoint_for(role));
        let model = m
            .model
            .clone()
            .unwrap_or_else(|| self.default_model_for(role));
        let key = m
            .key_env
            .as_ref()
            .and_then(|key_env| self.lookup_env(key_env))
            .or_else(|| no_auth_key_for_local(&endpoint))?;
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
    "missing large-model API key. Provide one of:\n  export SIFT_API_KEY=<KEY>\n  .env:\n    SIFT_API_KEY_ENV=WJT_AZURE_OPENAI_API_KEY\n  sift ./repo --api-key-file ~/.sift/key\n  ~/.sift/config.toml:\n    api_key = \"<KEY>\"\n  ~/.sift/config.toml:\n    [[model]]\n    role = \"large\"\n    key_env = \"SIFT_API_KEY\"\nOr use --scan-only for scan/dehydrate only."
}

pub fn run_doctor() -> bool {
    let mut doctor = Doctor::default();
    doctor.info("runtime", "sift doctor does not print secret values");

    let Some(path) = config_path() else {
        doctor.fail(
            "config",
            "HOME is not set; cannot resolve ~/.sift/config.toml",
        );
        doctor.print();
        return false;
    };
    doctor.info("config", &format!("path {}", path.display()));

    let src = match fs::read_to_string(&path) {
        Ok(src) => {
            doctor.pass("config", "file exists");
            src
        }
        Err(e) if e.kind() == ErrorKind::NotFound => match create_default_config(&path) {
            Ok(()) => {
                doctor.warn(
                    "config",
                    "file was missing; created default non-secret config",
                );
                DEFAULT_CONFIG.to_string()
            }
            Err(e) => {
                doctor.fail("config", &format!("cannot create default config: {e}"));
                doctor.print();
                return false;
            }
        },
        Err(e) => {
            doctor.fail("config", &format!("cannot read config: {e}"));
            doctor.print();
            return false;
        }
    };

    check_config_permissions(&path, &mut doctor);

    let file = match parse_file_config(&src) {
        Ok(file) => {
            doctor.pass("config", "TOML parses");
            file
        }
        Err(e) => {
            doctor.fail("config", &format!("invalid TOML: {e}"));
            doctor.print();
            return false;
        }
    };

    check_file_config(&file, &mut doctor);
    doctor.print();
    !doctor.has_fail
}

#[derive(Default)]
struct Doctor {
    rows: Vec<DoctorRow>,
    has_fail: bool,
}

struct DoctorRow {
    status: &'static str,
    area: &'static str,
    detail: String,
}

impl Doctor {
    fn pass(&mut self, area: &'static str, detail: &str) {
        self.push("PASS", area, detail);
    }
    fn warn(&mut self, area: &'static str, detail: &str) {
        self.push("WARN", area, detail);
    }
    fn fail(&mut self, area: &'static str, detail: &str) {
        self.has_fail = true;
        self.push("FAIL", area, detail);
    }
    fn info(&mut self, area: &'static str, detail: &str) {
        self.push("INFO", area, detail);
    }
    fn push(&mut self, status: &'static str, area: &'static str, detail: &str) {
        self.rows.push(DoctorRow {
            status,
            area,
            detail: detail.to_string(),
        });
    }
    fn print(&self) {
        println!("# sift doctor\n");
        println!("| Status | Area | Detail |");
        println!("|---|---|---|");
        for row in &self.rows {
            println!(
                "| {} | `{}` | {} |",
                row.status,
                row.area,
                escape_markdown_cell(&row.detail)
            );
        }
    }
}

fn check_config_permissions(path: &Path, doctor: &mut Doctor) {
    let Ok(meta) = fs::metadata(path) else {
        doctor.warn("config", "cannot inspect file permissions");
        return;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 == 0 {
            doctor.pass("config", &format!("permissions {:03o}", mode));
        } else {
            doctor.warn(
                "config",
                &format!(
                    "permissions {:03o}; consider chmod 600 ~/.sift/config.toml",
                    mode
                ),
            );
        }
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        doctor.info(
            "config",
            "permission check is not implemented on this platform",
        );
    }
}

fn check_file_config(file: &FileConfig, doctor: &mut Doctor) {
    if file.api_key.is_some() {
        doctor.warn(
            "secrets",
            "api_key is set in config; prefer key_env or --api-key-file",
        );
    }

    if file.models.is_empty() {
        check_legacy_large_config(file, doctor);
        return;
    }

    let mut large_available = false;
    let mut large_declared = false;
    for (idx, model) in file.models.iter().enumerate() {
        let role = model.role.unwrap_or(Role::Large);
        let label = match role {
            Role::Small => "small",
            Role::Large => {
                large_declared = true;
                "large"
            }
        };
        let endpoint = model.endpoint.as_deref().unwrap_or(match role {
            Role::Small => file.small_endpoint.as_deref().unwrap_or(DEFAULT_ENDPOINT),
            Role::Large => file.endpoint.as_deref().unwrap_or(DEFAULT_ENDPOINT),
        });
        let model_name = model.model.as_deref().unwrap_or(match role {
            Role::Small => file.small_model.as_deref().unwrap_or(DEFAULT_SMALL_MODEL),
            Role::Large => file.model.as_deref().unwrap_or(DEFAULT_LARGE_MODEL),
        });
        doctor.info(
            "model",
            &format!("model[{idx}] role={label} endpoint={endpoint} model={model_name}"),
        );

        match model.key_env.as_deref() {
            Some(key_env) if env_exists(key_env) => {
                doctor.pass("secrets", &format!("model[{idx}] key_env {key_env} is set"));
                if role == Role::Large {
                    large_available = true;
                }
                check_endpoint_key_pair(idx, role, endpoint, model_name, key_env, doctor);
            }
            Some(key_env) => {
                if is_local_endpoint(endpoint) {
                    doctor.pass(
                        "secrets",
                        &format!(
                            "model[{idx}] key_env {key_env} is not set; local endpoint will run without auth"
                        ),
                    );
                    if role == Role::Large {
                        large_available = true;
                    }
                    check_endpoint_key_pair(idx, role, endpoint, model_name, key_env, doctor);
                } else {
                    let msg =
                        format!("model[{idx}] key_env {key_env} is not set; this model is skipped");
                    match role {
                        Role::Small => doctor.warn("secrets", &msg),
                        Role::Large => doctor.fail("secrets", &msg),
                    }
                }
            }
            None => {
                let fallback = env_exists(ENV_API_KEY) || file.api_key.is_some();
                if role == Role::Large && fallback {
                    doctor.pass(
                        "secrets",
                        &format!("model[{idx}] uses fallback {ENV_API_KEY} or api_key"),
                    );
                    large_available = true;
                } else if is_local_endpoint(endpoint) {
                    doctor.pass(
                        "secrets",
                        &format!(
                            "model[{idx}] has no key_env; local endpoint will run without auth"
                        ),
                    );
                    if role == Role::Large {
                        large_available = true;
                    }
                } else {
                    let msg = format!("model[{idx}] has no key_env; this model is skipped");
                    match role {
                        Role::Small => doctor.warn("secrets", &msg),
                        Role::Large => doctor.fail("secrets", &msg),
                    }
                }
            }
        }
    }

    if !large_declared {
        check_legacy_large_config(file, doctor);
    } else if !large_available {
        doctor.fail(
            "model",
            "no usable large model; full audit will fail before convergence",
        );
    }
}

fn check_legacy_large_config(file: &FileConfig, doctor: &mut Doctor) {
    let endpoint = file.endpoint.as_deref().unwrap_or(DEFAULT_ENDPOINT);
    let model = file.model.as_deref().unwrap_or(DEFAULT_LARGE_MODEL);
    doctor.info(
        "model",
        &format!("legacy large endpoint={endpoint} model={model}"),
    );
    if env_exists(ENV_API_KEY) || file.api_key.is_some() {
        doctor.pass("secrets", &format!("{ENV_API_KEY} or api_key is available"));
    } else {
        doctor.fail(
            "secrets",
            &format!("missing large-model key; set {ENV_API_KEY} or add [[model]].key_env"),
        );
    }
}

fn check_endpoint_key_pair(
    idx: usize,
    role: Role,
    endpoint: &str,
    model_name: &str,
    key_env: &str,
    doctor: &mut Doctor,
) {
    if is_public_openai_endpoint(endpoint) && looks_like_azure_key_env(key_env) {
        doctor.fail(
            "endpoint",
            &format!(
                "model[{idx}] uses api.openai.com with Azure-looking key_env {key_env}; this commonly returns 401"
            ),
        );
    }
    if is_azure_endpoint(endpoint) {
        doctor.pass(
            "endpoint",
            &format!("model[{idx}] endpoint looks Azure; transport will send api-key header"),
        );
    }
    if role == Role::Large && is_public_openai_endpoint(endpoint) && model_name.contains("5.5") {
        doctor.warn(
            "model",
            &format!("model[{idx}] name {model_name} may be deployment-specific; verify it exists on api.openai.com"),
        );
    }
    if role == Role::Small && is_local_endpoint(endpoint) {
        doctor.info(
            "endpoint",
            &format!("model[{idx}] is local; no auth header is required"),
        );
    }
}

fn no_auth_key_for_local(endpoint: &str) -> Option<String> {
    is_local_endpoint(endpoint).then(String::new)
}

fn env_exists(key: &str) -> bool {
    std::env::var(key).map(|v| !v.is_empty()).unwrap_or(false)
}

fn is_public_openai_endpoint(endpoint: &str) -> bool {
    endpoint.contains("api.openai.com")
}

fn is_azure_endpoint(endpoint: &str) -> bool {
    endpoint.contains(".openai.azure.com") || endpoint.contains(".cognitiveservices.azure.com")
}

fn is_local_endpoint(endpoint: &str) -> bool {
    endpoint.contains("://localhost")
        || endpoint.contains("://127.0.0.1")
        || endpoint.contains("://[::1]")
}

fn looks_like_azure_key_env(key_env: &str) -> bool {
    key_env.to_ascii_uppercase().contains("AZURE")
}

fn escape_markdown_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn default_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

fn config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(config_path_from_home)
}

fn config_path_from_home(home: impl Into<PathBuf>) -> PathBuf {
    home.into().join(".sift/config.toml")
}

fn load_file_config() -> Result<FileConfig> {
    let Some(path) = config_path() else {
        return Ok(FileConfig::default());
    };
    let src = match std::fs::read_to_string(&path) {
        Ok(src) => src,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            create_default_config(&path)
                .with_context(|| format!("create default config file {}", path.display()))?;
            DEFAULT_CONFIG.to_string()
        }
        Err(e) => return Err(anyhow!("cannot read config file {}: {e}", path.display())),
    };
    parse_file_config(&src).map_err(|e| anyhow!("invalid config file {}: {e}", path.display()))
}

fn create_default_config(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create config directory {}", parent.display()))?;
    }
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("open config file {}", path.display())),
    };
    file.write_all(DEFAULT_CONFIG.as_bytes())
        .with_context(|| format!("write config file {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod config file {}", path.display()))?;
    }
    Ok(())
}

fn load_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let src = match std::fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(anyhow!("cannot read env file {}: {e}", path.display())),
    };
    parse_env_file(&src).with_context(|| format!("invalid env file {}", path.display()))
}

fn load_policy_config(path: &Path) -> Result<Policy> {
    let src = match std::fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Policy::default()),
        Err(e) => return Err(anyhow!("cannot read policy file {}: {e}", path.display())),
    };
    parse_policy_config(&src).with_context(|| format!("invalid policy file {}", path.display()))
}

fn parse_policy_config(src: &str) -> Result<Policy> {
    let parsed: toml::Value = toml::from_str(src).context("parse TOML")?;
    let table = parsed
        .as_table()
        .ok_or_else(|| anyhow!("policy root must be a TOML table"))?;
    let mut policy = Policy {
        max_candidate_files: optional_usize(table, "max_candidate_files")?,
        ..Policy::default()
    };
    policy.allowlist = optional_policy_matchers(table, "allowlist")?;
    policy.denylist = optional_policy_matchers(table, "denylist")?;
    policy.severity_overrides = optional_policy_severity_overrides(table, "severity_override")?;
    Ok(policy)
}

fn optional_policy_matchers(table: &toml::Table, key: &str) -> Result<Vec<PolicyMatcher>> {
    let Some(value) = table.get(key) else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| anyhow!("policy key {key} must be an array of tables"))?;
    let mut out = Vec::new();
    for item in values {
        let t = item
            .as_table()
            .ok_or_else(|| anyhow!("policy key {key} entries must be tables"))?;
        let matcher = PolicyMatcher {
            path: optional_string(t, "path")?,
            rule: optional_string(t, "rule")?,
            reason: optional_string(t, "reason")?,
        };
        if matcher.path.is_none() && matcher.rule.is_none() {
            return Err(anyhow!("policy key {key} entries require path or rule"));
        }
        out.push(matcher);
    }
    Ok(out)
}

fn optional_policy_severity_overrides(
    table: &toml::Table,
    key: &str,
) -> Result<Vec<PolicySeverityOverride>> {
    let Some(value) = table.get(key) else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| anyhow!("policy key {key} must be an array of tables"))?;
    let mut out = Vec::new();
    for item in values {
        let t = item
            .as_table()
            .ok_or_else(|| anyhow!("policy key {key} entries must be tables"))?;
        let severity = optional_string(t, "severity")?
            .ok_or_else(|| anyhow!("policy key {key} entries require severity"))?;
        if !matches!(severity.as_str(), "high" | "medium" | "low") {
            return Err(anyhow!(
                "policy key {key}.severity must be high, medium, or low"
            ));
        }
        let matcher = PolicySeverityOverride {
            path: optional_string(t, "path")?,
            rule: optional_string(t, "rule")?,
            severity,
            reason: optional_string(t, "reason")?,
        };
        if matcher.path.is_none() && matcher.rule.is_none() {
            return Err(anyhow!("policy key {key} entries require path or rule"));
        }
        out.push(matcher);
    }
    Ok(out)
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

fn validate_price(name: &str, value: Option<f64>) -> Result<()> {
    if let Some(price) = value
        && (!price.is_finite() || price < 0.0)
    {
        return Err(anyhow!("{name} must be a non-negative finite number"));
    }
    Ok(())
}

fn read_key_file(path: &Path) -> Result<String> {
    let key = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("cannot read api key file {}: {e}", path.display()))?;
    let key = key.trim();
    if key.is_empty() {
        Err(anyhow!("api key file {} is empty", path.display()))
    } else {
        Ok(key.to_string())
    }
}

fn parse_file_config(src: &str) -> Result<FileConfig> {
    let parsed: toml::Value = toml::from_str(src).context("parse TOML")?;
    let table = parsed
        .as_table()
        .ok_or_else(|| anyhow!("config root must be a TOML table"))?;
    let mut cfg = FileConfig {
        api_key: optional_string(table, "api_key")?,
        concurrency: optional_usize(table, "concurrency")?,
        max_bytes: optional_u64(table, "max_bytes")?,
        ignores: optional_string_array(table, "ignores")?,
        endpoint: optional_string(table, "endpoint")?,
        model: optional_legacy_model_string(table)?,
        small_endpoint: optional_string(table, "small_endpoint")?,
        small_model: optional_string(table, "small_model")?,
        timeout_ms: optional_u64(table, "timeout_ms")?,
        max_retries: optional_u32(table, "max_retries")?,
        models: Vec::new(),
    };

    if let Some(models) = table.get("model").and_then(|v| v.as_array()) {
        for item in models {
            let t = item
                .as_table()
                .ok_or_else(|| anyhow!("model entries must be TOML tables"))?;
            let role = match optional_string(t, "role")? {
                Some(role) => Some(
                    parse_role(&role)
                        .ok_or_else(|| anyhow!("model.role must be small or large, got {role}"))?,
                ),
                None => None,
            };
            let model = FileModelConfig {
                role,
                endpoint: optional_string(t, "endpoint")?,
                model: optional_string(t, "model")?,
                key_env: optional_string(t, "key_env")?,
                timeout_ms: optional_u64(t, "timeout_ms")?,
                max_retries: optional_u32(t, "max_retries")?,
            };
            if model.role.is_some() {
                cfg.models.push(model);
            }
        }
    }

    Ok(cfg)
}

fn optional_legacy_model_string(table: &toml::Table) -> Result<Option<String>> {
    let Some(value) = table.get("model") else {
        return Ok(None);
    };
    if value.is_array() {
        return Ok(None);
    }
    value
        .as_str()
        .map(|s| Some(s.to_string()))
        .ok_or_else(|| anyhow!("config key model must be a string or [[model]] table array"))
}

fn optional_string(table: &toml::Table, key: &str) -> Result<Option<String>> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    value
        .as_str()
        .map(|s| Some(s.to_string()))
        .ok_or_else(|| anyhow!("config key {key} must be a string"))
}

fn optional_u64(table: &toml::Table, key: &str) -> Result<Option<u64>> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    match value.as_integer() {
        Some(i) => u64::try_from(i)
            .map(Some)
            .map_err(|_| anyhow!("config key {key} must be a non-negative integer")),
        None => Err(anyhow!("config key {key} must be an integer")),
    }
}

fn optional_usize(table: &toml::Table, key: &str) -> Result<Option<usize>> {
    optional_u64(table, key).and_then(|value| match value {
        Some(v) => usize::try_from(v)
            .map(Some)
            .map_err(|_| anyhow!("config key {key} is too large")),
        None => Ok(None),
    })
}

fn optional_u32(table: &toml::Table, key: &str) -> Result<Option<u32>> {
    optional_u64(table, key).and_then(|value| match value {
        Some(v) => u32::try_from(v)
            .map(Some)
            .map_err(|_| anyhow!("config key {key} is too large")),
        None => Ok(None),
    })
}

fn optional_string_array(table: &toml::Table, key: &str) -> Result<Option<Vec<String>>> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    let values = value
        .as_array()
        .ok_or_else(|| anyhow!("config key {key} must be an array of strings"))?;
    let mut out = Vec::new();
    for item in values {
        let Some(s) = item.as_str() else {
            return Err(anyhow!("config key {key} must be an array of strings"));
        };
        out.push(s.to_string());
    }
    Ok(Some(out))
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
    use clap::CommandFactory;

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
    fn valid_toml_wrong_types_reject_config_not_silent_default() {
        let err = parse_file_config("concurrency=\"oops\"\nmax_bytes=\"huge\"\n");
        assert!(err.is_err());
        let err = parse_file_config("ignores=[\"target\", 1]\n");
        assert!(err.is_err());
        let err = parse_file_config("model=42\n");
        assert!(err.is_err());
    }

    #[test]
    fn default_config_template_is_valid_toml() {
        let cfg = parse_file_config(DEFAULT_CONFIG).unwrap_or_default();
        assert_eq!(cfg.max_bytes, Some(512 * 1024));
        assert_eq!(cfg.model.as_deref(), Some(DEFAULT_LARGE_MODEL));
        assert_eq!(cfg.small_model.as_deref(), Some(DEFAULT_SMALL_MODEL));
        assert!(cfg.api_key.is_none());
    }

    #[test]
    fn default_config_path_is_home_sift() {
        assert_eq!(
            config_path_from_home(PathBuf::from("/tmp/home")),
            PathBuf::from("/tmp/home/.sift/config.toml")
        );
    }

    #[test]
    fn creates_default_config_file() {
        let root = unique_test_dir("default-config");
        let path = root.join(".sift/config.toml");
        create_default_config(&path).unwrap_or_default();
        let src = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(src.contains("sift default configuration"));
        assert!(parse_file_config(&src).is_ok());
        std::fs::remove_dir_all(root).ok();
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
    fn rejects_unknown_model_role() {
        let err = parse_file_config("[[model]]\nrole=\"medium\"\nmodel=\"x\"\n");
        assert!(err.is_err());
    }

    #[test]
    fn rejects_wrong_types_inside_model_blocks() {
        let err = parse_file_config("[[model]]\nrole=\"small\"\ntimeout_ms=\"fast\"\n");
        assert!(err.is_err());
    }

    #[test]
    fn explicit_api_key_file_must_be_readable_and_non_empty() {
        let root = unique_test_dir("key-file");
        std::fs::create_dir_all(&root).ok();
        let missing = root.join("missing-key");
        let err = read_key_file(&missing).err().map(|e| e.to_string());
        assert!(err.unwrap_or_default().contains("cannot read api key file"));

        let empty = root.join("empty-key");
        std::fs::write(&empty, " \n").ok();
        let err = read_key_file(&empty).err().map(|e| e.to_string());
        assert!(err.unwrap_or_default().contains("is empty"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn concurrency_never_zero() {
        assert!(default_concurrency() >= 1);
    }

    #[test]
    fn cli_parses_report_language_and_debug() {
        let parsed = Cli::try_parse_from(["sift", "--report-language", "zh", "--debug"]);
        assert!(parsed.is_ok());
        let cli = match parsed {
            Ok(cli) => cli,
            Err(_) => return,
        };
        assert_eq!(cli.report_language, ReportLanguage::Zh);
        assert!(cli.debug);
    }

    #[test]
    fn cli_parses_benchmark_pricing_flags() {
        let parsed = Cli::try_parse_from([
            "sift",
            "--benchmark",
            "--benchmark-input-1m-cost",
            "0.25",
            "--benchmark-output-1m-cost",
            "1.00",
            "--benchmark-estimated-output-tokens",
            "1000",
        ]);
        assert!(parsed.is_ok());
        let cli = match parsed {
            Ok(cli) => cli,
            Err(_) => return,
        };
        assert!(cli.benchmark);
        assert_eq!(cli.benchmark_input_1m_cost, Some(0.25));
        assert_eq!(cli.benchmark_output_1m_cost, Some(1.0));
        assert_eq!(cli.benchmark_estimated_output_tokens, Some(1000));
    }

    #[test]
    fn cli_parses_github_subcommand_modes() {
        let parsed = Cli::try_parse_from([
            "sift",
            "github",
            "owner/repo",
            "--ref",
            "main",
            "--agent-gate",
            "--no-build",
            "--no-install",
        ]);
        assert!(parsed.is_ok());
        let cli = match parsed {
            Ok(cli) => cli,
            Err(_) => return,
        };
        assert!(matches!(cli.command, Some(CliCommand::Github(_))));
        let Some(CliCommand::Github(github)) = cli.command else {
            return;
        };
        assert_eq!(github.repo, "owner/repo");
        assert_eq!(github.ref_name.as_deref(), Some("main"));
        assert!(github.agent_gate);
        assert!(github.no_build);
        assert!(github.no_install);
    }

    #[test]
    fn self_audit_flag_is_not_public_cli_argument() {
        let help = Cli::command().render_long_help().to_string();
        assert!(!help.contains("--self-audit"));
        assert!(Cli::try_parse_from(["sift", "--self-audit"]).is_err());
    }

    #[test]
    fn missing_key_hint_uses_parseable_model_block() {
        let hint = missing_large_key_hint();
        assert!(hint.contains("[[model]]\n    role = \"large\""));
        assert!(!hint.contains("--self-audit"));
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
    fn local_model_can_omit_key_env() {
        let file = parse_file_config(
            r#"
[[model]]
role = "small"
endpoint = "http://127.0.0.1:11434/v1/chat/completions"
model = "gpt-oss"
"#,
        )
        .unwrap_or_default();
        let cfg = Config {
            root: PathBuf::new(),
            api_key: None,
            concurrency: 1,
            max_bytes: 1,
            ignores: Vec::new(),
            scan_only: false,
            agent_gate: false,
            format: OutputFormat::Text,
            benchmark: false,
            benchmark_output: None,
            benchmark_input_1m_cost: None,
            benchmark_output_1m_cost: None,
            benchmark_estimated_output_tokens: 0,
            endpoint: DEFAULT_ENDPOINT.to_string(),
            model: DEFAULT_LARGE_MODEL.to_string(),
            small_endpoint: DEFAULT_ENDPOINT.to_string(),
            small_model: DEFAULT_SMALL_MODEL.to_string(),
            timeout_ms: 1,
            max_retries: 0,
            report_language: ReportLanguage::En,
            debug: false,
            save: false,
            save_to: None,
            policy: Policy::default(),
            models: file.models,
            env_file: BTreeMap::new(),
        };
        let registry = cfg.build_registry();
        assert_eq!(registry.small.len(), 1);
    }

    #[test]
    fn absolute_module_must_stay_inside_target() {
        let root = unique_test_dir("module-root");
        let outside = unique_test_dir("module-outside");
        std::fs::create_dir_all(root.join("src")).ok();
        std::fs::create_dir_all(&outside).ok();
        let cli = Cli {
            command: None,
            target: root.clone(),
            module: Some(outside),
            api_key_file: None,
            concurrency: None,
            max_bytes: None,
            scan_only: true,
            agent_gate: false,
            format: OutputFormat::Text,
            benchmark: false,
            benchmark_output: None,
            benchmark_input_1m_cost: None,
            benchmark_output_1m_cost: None,
            benchmark_estimated_output_tokens: None,
            report_language: ReportLanguage::En,
            debug: false,
            save: false,
            save_to: None,
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
            command: None,
            target: root.clone(),
            module: Some(module.clone()),
            api_key_file: None,
            concurrency: None,
            max_bytes: None,
            scan_only: true,
            agent_gate: false,
            format: OutputFormat::Text,
            benchmark: false,
            benchmark_output: None,
            benchmark_input_1m_cost: None,
            benchmark_output_1m_cost: None,
            benchmark_estimated_output_tokens: None,
            report_language: ReportLanguage::En,
            debug: false,
            save: false,
            save_to: None,
        };
        let cfg = Config::resolve(cli).unwrap_or_else(|_| Config {
            root: PathBuf::new(),
            api_key: None,
            concurrency: 1,
            max_bytes: 1,
            ignores: Vec::new(),
            scan_only: true,
            agent_gate: false,
            format: OutputFormat::Text,
            benchmark: false,
            benchmark_output: None,
            benchmark_input_1m_cost: None,
            benchmark_output_1m_cost: None,
            benchmark_estimated_output_tokens: 0,
            endpoint: String::new(),
            model: String::new(),
            small_endpoint: String::new(),
            small_model: String::new(),
            timeout_ms: 1,
            max_retries: 0,
            report_language: ReportLanguage::En,
            debug: false,
            save: false,
            save_to: None,
            policy: Policy::default(),
            models: Vec::new(),
            env_file: BTreeMap::new(),
        });
        assert_eq!(cfg.root, module.canonicalize().unwrap_or(module));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn scan_only_and_agent_gate_cannot_be_combined() {
        let root = unique_test_dir("agent-gate-conflict");
        std::fs::create_dir_all(&root).ok();
        let cli = Cli {
            command: None,
            target: root.clone(),
            module: None,
            api_key_file: None,
            concurrency: None,
            max_bytes: None,
            scan_only: true,
            agent_gate: true,
            format: OutputFormat::Text,
            benchmark: false,
            benchmark_output: None,
            benchmark_input_1m_cost: None,
            benchmark_output_1m_cost: None,
            benchmark_estimated_output_tokens: None,
            report_language: ReportLanguage::En,
            debug: false,
            save: false,
            save_to: None,
        };

        let err = Config::resolve(cli).err().map(|e| e.to_string());

        assert!(err.unwrap_or_default().contains("--agent-gate"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn benchmark_mode_is_exclusive_and_prices_are_validated() {
        let root = unique_test_dir("benchmark-conflict");
        std::fs::create_dir_all(&root).ok();
        let cli = Cli {
            command: None,
            target: root.clone(),
            module: None,
            api_key_file: None,
            concurrency: None,
            max_bytes: None,
            scan_only: true,
            agent_gate: false,
            format: OutputFormat::Text,
            benchmark: true,
            benchmark_output: None,
            benchmark_input_1m_cost: None,
            benchmark_output_1m_cost: None,
            benchmark_estimated_output_tokens: None,
            report_language: ReportLanguage::En,
            debug: false,
            save: false,
            save_to: None,
        };
        let err = Config::resolve(cli).err().map(|e| e.to_string());
        assert!(err.unwrap_or_default().contains("--benchmark"));

        let cli = Cli {
            command: None,
            target: root.clone(),
            module: None,
            api_key_file: None,
            concurrency: None,
            max_bytes: None,
            scan_only: false,
            agent_gate: false,
            format: OutputFormat::Text,
            benchmark: true,
            benchmark_output: None,
            benchmark_input_1m_cost: Some(-0.1),
            benchmark_output_1m_cost: None,
            benchmark_estimated_output_tokens: None,
            report_language: ReportLanguage::En,
            debug: false,
            save: false,
            save_to: None,
        };
        let err = Config::resolve(cli).err().map(|e| e.to_string());
        assert!(err.unwrap_or_default().contains("benchmark-input-1m-cost"));
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

    #[test]
    fn parses_policy_schema_and_rejects_bad_severity() {
        let policy = parse_policy_config(
            r#"
max_candidate_files = 10

[[allowlist]]
path = "tests/fixtures"
rule = "download-execute"
reason = "synthetic"

[[severity_override]]
rule = "unpinned-github-action"
severity = "low"
"#,
        )
        .unwrap_or_default();
        assert_eq!(policy.max_candidate_files, Some(10));
        assert_eq!(policy.allowlist.len(), 1);
        assert_eq!(policy.severity_overrides[0].severity, "low");

        assert!(
            parse_policy_config("[[severity_override]]\nrule=\"x\"\nseverity=\"urgent\"\n")
                .is_err()
        );
    }
}
