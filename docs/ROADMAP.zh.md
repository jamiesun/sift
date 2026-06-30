# sift 项目画像与开发路线图

> [English](ROADMAP.md) | 中文
>
> 北极星 + 护栏 + 分阶段施工边界。说清"做成什么样 / 绝不做什么 / 每阶段交付什么 / 内部门禁何时生效"。
> 项目名：**sift**（CLI 即 `sift`）。语言：Rust。

## 项目概述

一个**可控成本**的开源项目审计器。引入开源库前，不必直接试用、也不必让前沿大模型生吞数万行代码，就能拿到一份定位到文件/行号的风险账本，据此决定是否引入。

核心是 **分级漏斗 + 算力错配 + ReACT 调度**：脏活（结构提取、粗筛压缩）交给零成本静态解析和廉价小模型；高强度逻辑收敛交给前沿大模型；一个 ReACT 状态机统一调度大小模型协同。整个工具编译为单一二进制，零配置即可审**整个项目**或**单个模块**。**sift 自身必须通过内部发布门禁。**

- 架构图

```text
  CLI key file / ENV / ~/.sift/config.toml ──(降级寻址, 缺 Key 即退)
        ▼
  扫描层  ignore::Walk → 有界 channel(消费即丢)         [P0 ✓]
        ▼
  零阶    tree-sitter 脱水(签名/import/调用) → JSON → drop AST  [P1 ✓]
        │  跨界引用打 [EXTERNAL_BLACKBOX]
        ▼
  模型层  多模型注册表 · 每调用硬超时 · 熔断+退避恢复    [P2 ✓]
        ▼
  ReACT 调度器(工具协议, 本地粗筛, retry≤N)              [P3 ✓]
        │  └─ 大模型(Reduce 收敛) ─────┘
        ▼
  报表层  stdout Markdown 风险清单(行号/调用链)           [P4 已启动]
        ▼
  内部门禁  源码评分检查 + 发布证据                         [P5/P6]
```

## 项目画像（目标状态）

- **零摩擦冷启动。** `sift ./repo --scan-only` 直接跑；缺 `~/.sift/config.toml` 时自动创建不含密钥的默认配置；不交互追问；缺 Key 立退给注入提示。
- **成本可控可预算。** 确定性 baseline 本地完成；完整审计才把脱水骨架交给大模型。
- **模型调度。** ReACT 状态机把本地粗筛与大模型收敛编排成一条链，技能是编译期写死的本地函数。
- **多模型 + 并发提速。** 可配置多个模型端点；扫描/模型并发保持有界且可观测。
- **绝不无脑死磕。** 每个外部调用有硬超时；连续失败触发熔断；熔断后退避恢复或降级，到顶则输出半成品而非挂死。
- **默认工程级。** 一份看起来完整但实际不完整的审计报告就是缺陷。跳过输入、截断、回退、半成品模型结果、无效配置都必须可见且可测试。
- **稳定机器契约。** 扫描 JSONL、最终 Markdown、诊断信息和生成报告各走清晰通道。下游脚本消费 stdout 时不应该猜里面是否混了多种格式。
- **内存与规模脱钩。** 流式处理、处理完即丢，常驻内存压低位。
- **内部门禁约束。** 项目必须通过维护者专用发布门禁；代码模块化、TDD 守护、边界清晰。
- **品质冲突优先级：** 鲁棒不崩 > 报表可用 > 成本低 > 速度快 > 体积小。

## 非目标（铁律）

- **不做向量库 / Embedding / RAG。** 单次低频审计，索引成本大于直接拼 prompt，纯文本管道阅后即焚。
- **不做运行时插件 / 动态技能注册。** 技能 = 编译期 enum + match 本地函数；扩展靠改码重编译。
- **不做服务化 / Web UI / 多租户。** 一次性 CLI，无常驻、无界面。
- **不允许 panic 主进程。** 脏数据静默丢弃记日志；幻觉/坏 JSON 走熔断；全程 Result/Option，无 unwrap/expect。
- **不允许无超时阻塞。** 任何子进程/网络/模型调用必须有 deadline，无界等待视为 bug。
- **模块审计不膨胀成全局。** 跨界引用打断点交大模型脑补，不盲目追链。
- **不靠"直接试用"替代审计。** 价值在引入前判断。
- **脚手架不得冒充产品能力。** 占位实现只能存在于明确未完成的阶段内；不能产出看起来像生产完成的报告。
- **不允许静默回退。** 无效配置、seed 截断、跳过文件、缺模型角色、模型路径降级，必须明确失败或写入报告。

## 模块化结构（Code Map）

> 每个 `src/*.rs` 自带单测；新子系统建则单测同建（TDD）。模块边界即责任边界，禁跨层乱伸手。

```text
src/main.rs       入口装配：解析→Config→调度→报表→退出码
src/config.rs     降级寻址、多模型配置加载            [P0✓→P2扩]
src/scanner.rs    Walk + 有界 channel                  [P0✓]
src/extract.rs    tree-sitter 脱水 → AstSummary        [P1]
src/model.rs      多模型注册表/客户端 trait/超时熔断    [P2✓]
src/react.rs      ReACT 状态机 + 技能 enum/match        [P3 ✓]
src/skills.rs     本地技能函数(粗筛/reduce收敛)         [P3 ✓→P4]
src/report.rs     Markdown 风险清单渲染                 [P4]
src/audit.rs      内部门禁维度评分(借鉴 scoot, 裁剪)    [P5]
```

## 多模型与并发（config schema）

```toml
concurrency = 8          # 扫描/模型并发上限
[[model]]                # 可多条；role 决定用途
role = "small"           # small=粗筛池 / large=收敛
endpoint = "..."
key_env = "SIFT_SMALL_KEY"
timeout_ms = 8000
max_retries = 1
[[model]]
role = "large"
endpoint = "..."
key_env = "SIFT_API_KEY"
timeout_ms = 60000
max_retries = 1
```
寻址降级：CLI key file > ENV > toml > 默认；无 large key 即退。小模型缺失可降级为纯 AST 兜底。
默认用户配置路径为 `~/.sift/config.toml`；首次运行从等价于 `config.example.toml` 的安全默认值创建，不能写入明文密钥。

## 超时熔断与恢复（绝不死磕）

- **每调用 deadline**：超时即弃，不无界等待。
- **熔断计数器**：单链连续失败/坏 JSON/未注册技能达阈值 → break，停 I/O。
- **退避恢复**：瞬时错指数退避重试到预算；非瞬时错降级（小模型回退 AST、大模型回退半成品）。
- **预算上限**：全局 token/时长封顶，触顶强制收敛输出 `[TRUNCATED]` 报表。

## 工程契约

- 标记完成的阶段必须有行为级证据，不能只有类型接线或 happy-path 单测。
- 完整审计 stdout 是最终报告流；`--scan-only` 是 JSONL 流；诊断信息不得进入 stdout。
- 报告必须披露输入覆盖：扫描、脱水、送入模型、跳过、截断的规模。
- 配置文件属于信任边界。用户配置缺失时从安全默认值自动创建；配置文件存在但无效时，进程必须失败，不能退回默认值。
- `src/` 下程序源码的运行时文本、prompt 和注释只用英文；双语文档保留在 docs。

## 阶段路线图

> 每阶段含：功能清单 / 边界约束 / 内部门禁。门禁全绿才进下阶段，并据门禁证据定下一步。

### P0 脚手架 — 已完成 ✓
- 功能：clap 降级寻址、有界通道扫描、缺 Key 熔断退出、最小装配。
- 边界：不连网、不解析、不留内存树。
- 门禁：`cargo build` 绿 / 0 unwrap / `--scan-only` 能扫 / 缺 Key exit1。

### P1 零阶 AST 脱水 — 已完成 ✓
- 功能：tree-sitter 接 Rust/Python/Go/JavaScript/TypeScript/HTML/CSS/Zig/Bash/Dart/Kotlin/Java/C/C++/C#/PHP/Swift/Ruby/SQL/Dockerfile/YAML/HCL/Vue/Svelte，提签名/import/调用，输出扁平 AstSummary JSON；跨界打 `[EXTERNAL_BLACKBOX]`；解析即 drop。
- 边界：丢注释与代码体；遇到残缺语法不 panic，并在下游报告披露覆盖不完整；不评价质量。
- 门禁：百兆库内存稳定低位、坏文件不崩；extract.rs 单测覆盖典型/残缺样本。

### P2 模型层（多模型+超时熔断） — 已完成 ✓
- 功能：ModelClient trait、注册表、role 路由；每调用硬超时、熔断、退避恢复；可配多端点。
- 边界：不写缓存、不持久化；密钥仅 env/文件、不入日志。
- 门禁：超时/坏响应有测试模拟，熔断必触发不死磕；无明文密钥。粗筛/收敛接线留 P3。

### P3 ReACT 调度器 — 已完成 ✓
- 功能：enum 状态机，初始工具协议提示，大模型出 `<TOOL_CALL>`，经 `$SEED` match 路由本地技能；retry≤N 熔断半成品。
- 边界：技能编译期写死；无动态加载。
- 门禁：注入坏 JSON/未注册技能/连错 N 次能熔断；react.rs 单测覆盖。

### P4 Map+Reduce+报表
- 功能：确定性 AST 粗筛账本、Markdown 渲染、真实 `[[model]]` TOML 解析、显式输入覆盖率、稳定 JSON 门禁、policy、artifact inventory、eval corpus 与干净 stdout 边界。
- 边界：模块审计只切根；跨界不追；截断和模型降级路径必须可见。
- 门禁：审已知样本命中预埋风险；模块/项目模式不串；完整审计 stdout 只含报告；无效配置失败；fake-endpoint full audit smoke 证明用户路径可用。

### P5 内部质量门禁
- 功能：audit.rs 跑裁剪维度评分，并把维护者专用报告写入 `reports/`(gitignore)。
- 门禁：硬规则无 FAIL/WARN，包括无 broad dead-code allow、无中文源码字符串/注释、报告流边界干净、seed 截断可见。

### P6 发布加固
- 功能：ReleaseSafe 单二进制、Makefile 安装路径、macOS Homebrew tap 发布、多语法扩展、JSON 输出稳定。
- 门禁：单文件分发、内部门禁通过、文档↔功能一致，`brew install jamiesun/tap/sift` 由 release checksum 支撑。

## 完成的样子

- 空配置可跑，自动创建 `~/.sift/config.toml`，缺 Key 即退给提示；不挂起。
- 百兆库内存稳定、扫坏不崩。
- 报表定位行号、含跨模块依赖与并发/资源风险，可直接拍板。
- 报表声明输入覆盖和截断状态；覆盖不完整时绝不能看起来像完整结论。
- 任一外部调用必超时；连错熔断出半成品而非死磕。
- 同一二进制审项目与 `--module` 子目录不串。
- 内部发布门禁无 FAIL，硬规则无 WARN。

> 建议非铁律：rayon/具体超时阈值/体积耗时数字按基准定，别当验收红线锁死。已确立铁律：单二进制、降级寻址、有界通道、硬超时熔断、无 unwrap、TDD、内部门禁达标。
