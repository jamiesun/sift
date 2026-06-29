# sift

可控成本的开源项目审计器：**分级漏斗 + 算力错配 + ReACT 调度**。引入开源库前，不必生吞数万行代码进前沿大模型，就能拿到定位到文件/行号的风险账本。

- 脏活（结构提取/粗筛）→ tree-sitter + 廉价小模型
- 逻辑收敛 → 前沿大模型，ReACT 状态机统一调度
- 单二进制、零配置、可审项目或模块；sift 自身经得起 sift 审计

详见 [docs/ROADMAP.md](docs/ROADMAP.md)。

## 用法

```sh
sift ./repo --scan-only        # 仅扫描层
sift ./repo --module src        # 审子模块
sift ./repo --api-key <KEY>     # 全链路（或 SIFT_API_KEY 环境变量）
```

状态：P0 脚手架（clap 降级寻址 + 有界通道扫描）。AST/模型/ReACT 层开发中。
