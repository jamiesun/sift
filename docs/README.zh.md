# sift

> [English](../README.md) | 中文

可控成本的开源项目审计器：**分级漏斗 + 算力错配 + ReACT 调度**。引入开源库前，不必生吞数万行代码进前沿大模型，就能拿到定位到文件/行号的风险账本。

- 脏活（结构提取/粗筛）→ tree-sitter + 廉价小模型
- 逻辑收敛 → 前沿大模型，ReACT 状态机统一调度
- 单二进制、零配置、可审项目或模块；sift 自身必须通过内部发布门禁

详见 [ROADMAP.zh.md](ROADMAP.zh.md)。

## 用法

```sh
sift ./repo --scan-only        # 仅扫描层
sift ./repo --agent-gate       # 确定性预运行门禁，无需模型 Key
sift ./repo --benchmark        # 扫描/模型预算 telemetry JSON，无需模型 Key
sift github owner/repo         # 安全 GitHub intake，默认 --agent-gate
sift github owner/repo --ref main --scan-only
sift ./repo --module src        # 审子模块
SIFT_API_KEY=<KEY> sift ./repo  # 全链路
sift ./repo --api-key-file ~/.sift/key
sift ./repo --report-language zh # 输出中文 Markdown 报告
sift ./repo --debug              # 向 stderr 打印更多诊断
sift doctor                    # 检查配置、key_env 与 endpoint/key 错配
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

确定性供应链规则目前会标记 npm 安装生命周期脚本、Rust `build.rs`
命令边界、shell/Dockerfile 下载后执行模式、base64 解码后执行流、
GitHub Actions secrets 与 shell block 的耦合，以及未 pin 到 commit
SHA 的 GitHub Actions。

`sift github` 接受 `owner/repo` 或 `https://github.com/owner/repo`，
用 `git` 获取临时 checkout，解析 commit SHA，然后对该 checkout 运行本地
scan/gate/benchmark 管线。它不会运行仓库代码、包管理器命令、build
script、hook、install 命令或 submodule。临时 checkout 默认清理；只有
需要人工查看取回的源码树时才使用 `--keep-checkout`。

首次运行时，sift 会自动创建 `~/.sift/config.toml` 默认配置文件。默认配置只包含非密钥项；模型密钥放在环境变量里，或通过 `--api-key-file` 传入。

完整审计的 stdout 只保留最终 Markdown 报告；进度、状态和 debug 诊断都走 stderr，长任务不会看起来像卡死，也不影响下游工具安全消费 stdout。

`--benchmark` 是本地 telemetry 模式，用于 release note 和成本核算。
它不会调用模型；默认向 stdout 输出稳定 JSON，也可以用
`--benchmark-output <path>` 写入文件。报告包含候选/脱水/跳过计数、
扫描耗时、可用的 resident memory 指标、seed 字节数、计划 Reduce
批次、模型调用计数、近似 token 数，以及可选 USD 成本估算。价格必须
显式传入，不会自动猜测：

```sh
sift ./repo --benchmark \
  --benchmark-input-1m-cost 0.25 \
  --benchmark-output-1m-cost 1.00 \
  --benchmark-estimated-output-tokens 2000
```

## 支持语言

扫描层目前支持 Rust、Python、Go、JavaScript、TypeScript/TSX、HTML、CSS、Zig、Bash 兼容 shell 文件（`.sh`、`.bash`、`.zsh`）、Dart、Kotlin、Java、C/C++、C#、PHP、Swift、Ruby、SQL、Dockerfile/Containerfile、YAML、HCL/Terraform、Vue、Svelte、`package.json`、Makefile 和 Markdown 安装片段。

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

## 测试样本

`tests/fixtures/repo-intake/` 包含合成的恶意与良性仓库树，用于
确定性 `--agent-gate` 回归测试。这些 fixture 命令只是惰性样例，
绝不能当作安装脚本执行。

macOS release 通过已有 tap 安装：

```sh
brew install jamiesun/tap/sift
```

状态：P0 脚手架 + P1 AST 脱水 + P2 模型层 + P3 ReACT 调度器（工具协议、编译期技能、retry→半成品）已完成。P4 进行中：本地 AST 风险账本、Markdown 渲染、`[[model]]` 配置解析与小模型 Map 波次已接线。内部发布门禁会为维护者在 `reports/` 下写入本地报告；下一步补预埋风险报表门禁与更强评分。
