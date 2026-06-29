mod config;
mod extract;
mod model;
mod react;
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

    // 铁律：非 scan-only 必须有 Key，缺失立即退出，不挂起、不交互。
    if !cfg.scan_only && cfg.api_key.is_none() {
        eprintln!(
            "未找到 API Key。注入方式（任一）：\n  --api-key <KEY>\n  export SIFT_API_KEY=<KEY>\n  ~/.config/sift/config.toml: api_key=\"<KEY>\"\n或加 --scan-only 仅跑扫描层。"
        );
        return ExitCode::FAILURE;
    }

    eprintln!("审计根: {}", cfg.root.display());
    eprintln!(
        "并发: {}  单文件上限: {}B  scan_only: {}",
        cfg.concurrency, cfg.max_bytes, cfg.scan_only
    );

    let mut reg = if cfg.scan_only {
        None
    } else {
        let r = cfg.build_registry();
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

    // ReACT 收敛：仅在有大模型时驱动；缺则降级仅出脱水流。
    if let Some(large) = reg.as_mut().and_then(|r| r.large.as_mut()) {
        match react::ReAct::default().run(large, &seed) {
            react::Outcome::Final(rep) => println!("\n# 审计结论\n{rep}"),
            react::Outcome::Partial(rep) => eprintln!("部分结论(降级/超界): {rep}"),
        }
    }
    ExitCode::SUCCESS
}
