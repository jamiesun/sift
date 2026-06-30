# sift Docs / sift 文档

## English

`sift` is a cost-controlled open-source project auditor for dependency and repository intake. It is built around a tiered funnel: static AST dehydration first, optional small-model filtering second, and large-model convergence only after the input has been reduced.

The primary product surface is a local CLI that can inspect a project path or safely fetch a GitHub repository before setup, install, build, or agent execution:

```sh
sift ./repo --agent-gate
sift ./repo --benchmark
sift github owner/repo --ref main --agent-gate
```

The deterministic agent gate emits a stable pre-run verdict:

```text
VERDICT: ACCEPT | CAUTION | REJECT | INCOMPLETE
SAFE_TO_AGENT_RUN: yes | no
```

- [English Overview](en/README.md)
- [Roadmap](ROADMAP.md)
- [Contributor Handbook](en/AGENT.md)

## 中文

`sift` 是一个面向依赖引入和仓库预审的可控成本开源项目审计器。它先做静态 AST 脱水，再按需使用小模型粗筛，最后只把压缩后的输入交给大模型收敛。

主要入口是本地 CLI：可以审本地项目，也可以在 setup、install、build 或 agent 执行之前安全获取 GitHub 仓库并做门禁判断。

```sh
sift ./repo --agent-gate
sift ./repo --benchmark
sift github owner/repo --ref main --agent-gate
```

确定性 agent gate 输出稳定预运行 verdict：

```text
VERDICT: ACCEPT | CAUTION | REJECT | INCOMPLETE
SAFE_TO_AGENT_RUN: yes | no
```

- [中文概览](README.zh.md)
- [路线图](ROADMAP.zh.md)
- [贡献者手册](AGENT.zh.md)
