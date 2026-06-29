# AGENT.md — sift 贡献者手册

> [English](../AGENT.md) | 中文
>
> 给参与 sift 的人与 agent 的实现手册：铁律、结构、习惯的事实来源。画像/边界见 [ROADMAP.zh.md](ROADMAP.zh.md)。

## sift 是什么

可控成本的单二进制开源审计器：tree-sitter 脱水 → 小模型粗筛(Map) → 大模型收敛(Reduce)，由 ReACT 状态机调度。审项目或单模块。**sift 必须通过 `sift .` 自审。**

## 铁律

1. **`src/` 内禁 `unwrap()`/`expect()`。** 脏数据走 Result/Option 分支丢弃+记日志；主进程绝不 panic。
2. **每个外部调用（子进程/网络/模型）必有硬超时。** 无界阻塞即 bug；连错触发熔断；熔断后退避/降级/出半成品，绝不死磕。
3. **单二进制、低依赖。** 无向量库、无 embedding/RAG、无数据库、无缓存；纯文本管道，阅后即焚。
4. **技能仅编译期写死。** 技能 = enum + match 本地函数；无动态加载、无运行时插件。
5. **流式、内存与规模脱钩。** 有界通道，脱水后即 drop AST，常驻内存压低位。
6. **密钥降级寻址。** CLI > ENV > config.toml > 默认；缺大模型 Key 立退给提示，绝不挂起或交互追问。
7. **密钥仅 env/文件。** 不编译进、不提交、不打印、不入日志。
8. **模块审计不膨胀成全局。** 跨界引用打 `[EXTERNAL_BLACKBOX]`，不追链。
9. **TDD。** 每个 `src/*.rs` 自带单测；新子系统单测同建。
10. **中英双语、默认英文。** 每文档有 ZH 副本(`docs/*.zh.md`)；英文为准，跨语言范围/命令/规则须一致。

> 任一铁律违反即自审 FAIL。

## 模块地图

| 路径 | 责任 | 阶段 |
|------|------|------|
| `src/main.rs` | 装配：解析→Config→调度→报表→退出码 | P0 ✓ |
| `src/config.rs` | 降级寻址、多模型配置 | P0 ✓→P2 |
| `src/scanner.rs` | Walk + 有界通道 | P0 ✓ |
| `src/extract.rs` | tree-sitter 脱水 → AstSummary | P1 |
| `src/model.rs` | 模型注册表/客户端/超时/熔断 | P2 |
| `src/react.rs` | ReACT 状态机 + 技能 match | P3 |
| `src/skills.rs` | 本地技能函数(map/reduce) | P3-4 |
| `src/report.rs` | Markdown 风险清单 | P4 |
| `src/audit.rs` | 自审计评分 | P5 |

## 工作流

```sh
cargo build                    # 必须绿
cargo test                     # 必须过
cargo fmt && cargo clippy      # 提交前清
grep -rn 'unwrap()\|expect(' src   # 必须为 0
sift .                         # 自审(P5+)须无 FAIL
```

- 一次提交一个关注点；带 `Co-authored-by: Copilot` trailer。
- 加功能前查是否越 ROADMAP 非目标；越界先改铁律。
- 阶段自审门禁不绿不算完成。

## 习惯

- 全程 Result/Option；每个等待都有界；临时数据尽早 drop。
- 模块各守责任，不跨层乱伸手。
- 优先生态 crate，但拒重依赖。
- 报表入 `reports/`(gitignore)；审计不脏化跟踪文件。
