# sift

> [English](../README.md) | 中文

可控成本的开源项目审计器：**分级漏斗 + 算力错配 + ReACT 调度**。引入开源库前，不必生吞数万行代码进前沿大模型，就能拿到定位到文件/行号的风险账本。

- 脏活（结构提取/粗筛）→ tree-sitter + 廉价小模型
- 逻辑收敛 → 前沿大模型，ReACT 状态机统一调度
- 单二进制、零配置、可审项目或模块；sift 自身经得起 sift 审计

详见 [ROADMAP.zh.md](ROADMAP.zh.md)。

## 用法

```sh
sift ./repo --scan-only        # 仅扫描层
sift ./repo --module src        # 审子模块
SIFT_API_KEY=<KEY> sift ./repo  # 全链路
sift ./repo --api-key-file ~/.config/sift/key
sift ./repo --self-audit        # 本地 P5 门禁，无需模型 Key
```

状态：P0 脚手架 + P1 AST 脱水 + P2 模型层 + P3 ReACT 调度器（工具协议、编译期技能、retry→半成品）已完成。P4 进行中：本地 AST 风险账本、Markdown 渲染、`[[model]]` 配置解析与小模型 Map 波次已接线。最小 P5 本地自审计已能写入 `reports/self-audit.md`；下一步补预埋风险报表门禁与更强评分。
