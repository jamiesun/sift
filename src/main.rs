mod config;
mod scanner;

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
    eprintln!("并发: {}  单文件上限: {}B  scan_only: {}", cfg.concurrency, cfg.max_bytes, cfg.scan_only);

    let rx = scanner::spawn_scan(&cfg);
    let mut count = 0usize;
    for path in rx {
        count += 1;
        let _ = extract_ast_placeholder(&path);
    }

    eprintln!("扫描完成，候选文件: {count}");
    println!("# sift 报表（占位）\n\n扫描根 `{}`，命中 {count} 个文件。AST/LLM 层待接入。", cfg.root.display());
    ExitCode::SUCCESS
}

fn extract_ast_placeholder(_path: &std::path::Path) -> Option<()> {
    None
}
