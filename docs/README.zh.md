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
sift ./repo --agent-gate       # 确定性预运行门禁，无需模型 Key
sift ./repo --module src        # 审子模块
SIFT_API_KEY=<KEY> sift ./repo  # 全链路
sift ./repo --api-key-file ~/.sift/key
sift ./repo --report-language zh # 输出中文 Markdown 报告
sift ./repo --debug              # 向 stderr 打印更多诊断
sift doctor                    # 检查配置、key_env 与 endpoint/key 错配
sift ./repo --self-audit        # 本地 P5 门禁，无需模型 Key
```

`--agent-gate` 是给 agent 和包装脚本使用的本地确定性 repo-intake
门禁。它只向 stdout 写入以下稳定契约：

```text
VERDICT: ACCEPT | CAUTION | REJECT | INCOMPLETE
WHY:
- <top evidence>
BLOCKERS:
- <file:line evidence or coverage blocker>
SAFE_TO_AGENT_RUN: yes | no
```

只有 `SAFE_TO_AGENT_RUN: yes` 时命令退出码为 `0`；`CAUTION`、`REJECT`
和 `INCOMPLETE` 都返回非零，方便调用方在 setup、install、build 或 run
之前停止。

首次运行时，sift 会自动创建 `~/.sift/config.toml` 默认配置文件。默认配置只包含非密钥项；模型密钥放在环境变量里，或通过 `--api-key-file` 传入。

完整审计的 stdout 只保留最终 Markdown 报告；进度、状态和 debug 诊断都走 stderr，长任务不会看起来像卡死，也不影响下游工具安全消费 stdout。

## 支持语言

扫描层目前支持 Rust、Python、Go、JavaScript、TypeScript/TSX、HTML、CSS、Zig、Bash 兼容 shell 文件（`.sh`、`.bash`、`.zsh`）、Dart、Kotlin、Java、C/C++、C#、PHP、Swift、Ruby、SQL、Dockerfile/Containerfile、YAML、HCL/Terraform、Vue 和 Svelte。

## 安装

源码构建：

```sh
make ci
make install
```

安装本地 git hooks：

```sh
make githooks-install
```

pre-commit hook 会在每次提交前运行 `make local-ci`。确需临时跳过时，可执行 `SIFT_SKIP_LOCAL_CI=1 git commit ...`。

macOS release 通过已有 tap 安装：

```sh
brew install jamiesun/tap/sift
```

状态：P0 脚手架 + P1 AST 脱水 + P2 模型层 + P3 ReACT 调度器（工具协议、编译期技能、retry→半成品）已完成。P4 进行中：本地 AST 风险账本、Markdown 渲染、`[[model]]` 配置解析与小模型 Map 波次已接线。最小 P5 本地自审计已能写入 `reports/self-audit.md`；下一步补预埋风险报表门禁与更强评分。
