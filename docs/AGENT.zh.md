# AGENT.md — sift 贡献者手册

> [English](en/AGENT.md) | 中文
>
> 给参与 sift 的人与 agent 的实现手册：铁律、结构、习惯的事实来源。画像/边界见 [ROADMAP.zh.md](ROADMAP.zh.md)。

## sift 是什么

可控成本的单二进制开源审计器：tree-sitter 脱水 → 小模型粗筛(Map) → 大模型收敛(Reduce)，由 ReACT 状态机调度。审项目或单模块。**sift 必须通过内部发布门禁。**

## 铁律

1. **`src/` 内禁 `unwrap()`/`expect()`。** 脏数据走 Result/Option 分支丢弃+记日志；主进程绝不 panic。
2. **每个外部调用（子进程/网络/模型）必有硬超时。** 无界阻塞即 bug；连错触发熔断；熔断后退避/降级/出半成品，绝不死磕。
3. **单二进制、低依赖。** 无向量库、无 embedding/RAG、无数据库、无缓存；纯文本管道，阅后即焚。
4. **技能仅编译期写死。** 技能 = enum + match 本地函数；无动态加载、无运行时插件。
5. **流式、内存与规模脱钩。** 有界通道，脱水后即 drop AST，常驻内存压低位。
6. **密钥降级寻址。** CLI key file > ENV > 项目 `.env` > `~/.sift/config.toml` > 默认；缺大模型 Key 立退给提示，绝不挂起或交互追问。缺用户配置时自动创建不含密钥的默认配置。
7. **密钥仅 env/文件。** 不编译进、不提交、不打印、不入日志。
8. **模块审计不膨胀成全局。** 跨界引用打 `[EXTERNAL_BLACKBOX]`，不追链。
9. **TDD。** 每个 `src/*.rs` 自带单测；新子系统单测同建。
10. **中英双语、默认英文。** 每文档有 ZH 副本(`docs/*.zh.md`)；英文为准，跨语言范围/命令/规则须一致。
11. **禁止玩具门禁或虚假能力声明。** 脚手架代码必须明确标成 scaffold，并隔离在显式模式后面；只有行为级门禁证明后，才能算阶段完成。
12. **输出契约稳定。** `--scan-only` 可以向 stdout 写 JSONL；完整审计的 stdout 只留给最终报告。进度、诊断、模型遥测走 stderr 或 reports，不能混进报告流。
13. **禁止静默降级。** 截断、跳过文件、模型回退、半成品报告、无效配置和解析失败，必须体现在输出、退出码或内部门禁证据里。无效配置文件必须失败，不能悄悄回默认值。
14. **程序源码只用英文。** `src/` 内运行时字符串、prompt 和源码注释使用英文；双语用户文档保留在 `docs/*.zh.md`。

> 任一铁律违反即内部门禁 FAIL。

## 模块地图

| 路径 | 责任 | 阶段 |
|------|------|------|
| `src/main.rs` | 装配：解析→Config→调度→报表→退出码 | P0 ✓ |
| `src/config.rs` | 降级寻址、多模型配置 | P0 ✓→P2 |
| `src/scanner.rs` | Walk + 有界通道 | P0 ✓ |
| `src/extract.rs` | tree-sitter 脱水 → AstSummary | P1 ✓ |
| `src/model.rs` | 模型注册表/客户端/超时/熔断 | P2 ✓ |
| `src/react.rs` | ReACT 状态机 + 技能 match | P3 ✓ |
| `src/skills.rs` | 本地技能函数(map/reduce) | P3 ✓→P4 |
| `src/report.rs` | Markdown 风险清单 | P4 |
| `src/audit.rs` | 内部门禁评分 | P5 |

## 工作流

```sh
cargo build                    # 必须绿
cargo test                     # 必须过
cargo fmt && cargo clippy      # 提交前清
make ci                        # 对齐本地发布门禁
rg 'unwrap\(|expect\(|panic!' src  # 必须为 0
rg '[\p{Han}]' src             # 必须为 0
```

- 一次提交一个关注点；带 `Co-authored-by: Copilot` trailer。
- 加功能前查是否越 ROADMAP 非目标；越界先改铁律。
- 阶段内部门禁和至少一个用户路径行为 smoke 不绿，不算完成。
- 如果阶段使用脚手架，文档和代码必须明确写出仍未完成的部分。

## 习惯

- 全程 Result/Option；每个等待都有界；临时数据尽早 drop。
- 模块各守责任，不跨层乱伸手。
- 优先生态 crate，但拒重依赖。
- 报表入 `reports/`(gitignore)；审计不脏化跟踪文件。
- 宁可明确失败，也不要输出一份看起来完整但实际不完整的审计报告。
