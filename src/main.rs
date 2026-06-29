mod audit;
mod config;
mod extract;
mod model;
mod react;
mod report;
mod scanner;
mod skills;

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;

use config::{Cli, Config};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cfg = match Config::resolve(cli) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("配置错误: {e}");
            return ExitCode::FAILURE;
        }
    };

    eprintln!("审计根: {}", cfg.root.display());
    eprintln!(
        "并发: {}  单文件上限: {}B  scan_only: {}  self_audit: {}",
        cfg.concurrency, cfg.max_bytes, cfg.scan_only, cfg.self_audit
    );

    if cfg.self_audit {
        match audit::write_self_audit(&cfg.project_root) {
            Ok(result) => {
                eprintln!(
                    "自审计报告: {}  FAIL: {}  WARN: {}",
                    result.path.display(),
                    result.failures,
                    result.warnings
                );
                println!("{}", result.markdown);
                return if result.failures == 0 {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                };
            }
            Err(e) => {
                eprintln!("自审计失败: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    let mut reg = if cfg.scan_only {
        None
    } else {
        let r = cfg.build_registry();
        // 铁律：非 scan-only 必须有 large 模型，缺失立即退出，不挂起、不交互。
        if !r.has_large() {
            eprintln!("{}", config::missing_large_key_hint());
            return ExitCode::FAILURE;
        }
        eprintln!(
            "模型层: large={} small_pool={} degraded={}",
            r.has_large(),
            r.small.len(),
            r.degraded()
        );
        Some(r)
    };

    let rx = scanner::spawn_scan(&cfg);
    let mut count = 0usize;
    let mut dehydrated = 0usize;
    let mut seed = String::new();
    const SEED_CAP: usize = 64 * 1024;
    let mut out = std::io::stdout().lock();
    for path in rx {
        count += 1;
        let Ok(src) = std::fs::read(&path) else {
            continue;
        };
        if let Some(sum) = extract::dehydrate(&path, &src) {
            dehydrated += 1;
            if let Ok(j) = serde_json::to_string(&sum) {
                if reg.is_some() && seed.len() < SEED_CAP {
                    seed.push_str(&j);
                    seed.push('\n');
                }
                // 下游管道(head/grep)关闭即 broken pipe，干净收尾而非 panic。
                if writeln!(out, "{j}").is_err() {
                    return ExitCode::SUCCESS;
                }
            }
        }
        // AST 在 dehydrate 内解析完即 drop；这里不驻留任何树。
    }

    eprintln!("扫描完成，候选文件: {count}  脱水: {dehydrated}");

    let react_seed = if let Some(small_obs) = reg
        .as_mut()
        .and_then(|r| r.map_small_pool(&seed, cfg.concurrency))
    {
        eprintln!("小模型 Map 完成，observation 字节: {}", small_obs.len());
        format!("SMALL_MODEL_OBSERVATIONS:\n{small_obs}\n\nAST_SEED:\n{seed}")
    } else {
        seed.clone()
    };

    // ReACT 收敛：仅在有大模型时驱动；缺则降级仅出脱水流。
    if let Some(large) = reg.as_mut().and_then(|r| r.large.as_mut()) {
        match react::ReAct::default().run(large, &react_seed) {
            react::Outcome::Final(rep) => println!("\n# 审计结论\n{rep}"),
            react::Outcome::Partial(rep) => {
                eprintln!("部分结论(降级/超界): {rep}");
                println!(
                    "\n# 本地降级审计结论\n{}",
                    report::markdown_from_seed(&seed)
                );
            }
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_hint_stays_available_to_main() {
        assert!(config::missing_large_key_hint().contains("SIFT_API_KEY"));
    }
}
