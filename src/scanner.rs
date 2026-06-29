use std::path::PathBuf;
use std::thread;

use crossbeam_channel::{Receiver, bounded};
use ignore::WalkBuilder;

use crate::config::Config;

/// 通道有界容量：磁盘 I/O 太快时阻塞生产者，给内存装避震器。
const CHANNEL_CAP: usize = 1024;

/// 启动文件流：在后台线程遍历，主流程从 Receiver 拉路径，消费即丢。
pub fn spawn_scan(cfg: &Config) -> Receiver<PathBuf> {
    let (tx, rx) = bounded::<PathBuf>(CHANNEL_CAP);

    let root = cfg.root.clone();
    let max_bytes = cfg.max_bytes;
    let ignores = cfg.ignores.clone();

    thread::spawn(move || {
        let mut builder = WalkBuilder::new(&root);
        builder.standard_filters(true).hidden(false);
        builder.filter_entry(move |e| {
            !e.file_name()
                .to_str()
                .map(|n| ignores.iter().any(|ig| ig == n))
                .unwrap_or(false)
        });

        for dent in builder.build() {
            let Ok(entry) = dent else { continue };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            if entry
                .metadata()
                .map(|m| m.len() > max_bytes)
                .unwrap_or(true)
            {
                continue;
            }
            if tx.send(entry.into_path()).is_err() {
                break;
            }
        }
    });

    rx
}
