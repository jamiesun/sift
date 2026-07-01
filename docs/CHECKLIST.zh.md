# sift 验收清单

> [English](CHECKLIST.md) | 中文
>
> 针对 [ROADMAP.md](ROADMAP.zh.md) 承诺的每一项功能与门禁的时间点验收快照，严格限定在其阶段边界（P0–P6）与非目标范围内；边界之外的内容本清单不予评分——铁律见 [AGENT.md](../docs/AGENT.zh.md)。
> 行号以下方快照提交为准，后续可能漂移；行号与函数/测试名冲突时，以函数/测试名为准。

## 快照信息

| | |
|---|---|
| 提交 | `f9a374b` —「fix agent gate issue regressions (#39)」（比标签 `v0.2.0` 多 4 个提交），另有本次会话的后续修复尚未提交（见[自我审计 dogfood 检查](#自我审计-dogfood-检查)） |
| 评估日期 | 2026-07-01 |
| `cargo build` | ✅ 通过 |
| `make ci`（`fmt-check` + `test` + `clippy -D warnings` + `internal-gate`） | ✅ 通过，退出码 0 |
| 测试 | ✅ 127 个通过，0 个失败（`src/**` 内 119 个单测 + `tests/*.rs` 内 8 个黑盒测试） |
| 内部质量门禁（`reports/internal-gate.md`） | ✅ 13/13 检查 PASS，0 WARN，0 FAIL |

## 图例

| 标记 | 含义 |
|---|---|
| ✅ **完成** | 已交付且有行为级证据支撑（本次评估中的一个通过测试和/或一次真实运行）——不只是类型层接线或 happy-path 单测。 |
| 🟡 **部分完成** | 已交付但范围受限、按设计暂未激活，或缺少行内注明的某一项证据点。 |
| ⏳ **待定** | 尚无固定目标、等待维护者决策，或 `ROADMAP.md` 中本就明确留白/开放式。 |
| ⬜ **未完成** | 承诺过但未实现，或完全没有自动化证据。 |

对应本清单要回答的三分类：**完成 = ✅**，**待定 = ⏳**，**未完成 = 🟡 / ⬜**。

`Type` 列：**F** = 功能条目，**G** = 门禁/验收标准条目，**B** = 边界约束，均逐字取自各阶段 ROADMAP.md 原文。

---

## P0 — 脚手架

ROADMAP 状态：**已完成 ✓**

| # | Type | 条目 | 状态 | 证据 |
|---|---|------|------|------|
| 1 | F | 降级寻址：CLI key file › ENV › 项目 `.env` › `~/.sift/config.toml` › 默认值 | ✅ 完成 | `src/config.rs::Config::resolve`；测试 `parses_project_env_file`、`explicit_api_key_file_must_be_readable_and_non_empty` |
| 2 | F | 有界通道扫描器（Walk → 有界 channel，消费即丢） | ✅ 完成 | `src/scanner.rs`（`crossbeam_channel::bounded::<PathBuf>(1024)`）；测试 `scan_skips_ignored_dirs_and_large_files` |
| 3 | F | 最小端到端装配：解析 → Config → 调度 → 报表 → 退出码 | ✅ 完成 | `src/main.rs::main` |
| 4 | G | `cargo build` 绿 | ✅ 完成 | 本次会话验证（`make ci` 退出码 0） |
| 5 | G | `src/` 内 0 处 `unwrap()`/`expect()` | ✅ 完成 | `reports/internal-gate.md`：「No direct unwrap/expect in src」— PASS |
| 6 | G | `--scan-only` 无需任何模型 Key 即可扫描 | ✅ 完成 | `tests/benchmark_mode.rs::scan_only_stdout_remains_jsonl_not_benchmark_json` |
| 7 | G | 完整审计缺大模型 Key 时在调度前退出 | 🟡 部分完成 | 代码路径已存在（`src/main.rs:83-86`、`config::missing_large_key_hint`），但单测只覆盖提示文案内容（`missing_key_hint_uses_parseable_model_block`）；**没有黑盒测试实际拉起二进制、在非 `--scan-only`/`--agent-gate`/`--benchmark` 路径下缺 Key 时断言进程退出码** |

**阶段结论：✅ 完成**，仅 #7 存在测试覆盖缺口。

---

## P1 — 零阶 AST 脱水

ROADMAP 状态：**已完成 ✓**

| # | Type | 条目 | 状态 | 证据 |
|---|---|------|------|------|
| 1 | F | tree-sitter 语法覆盖：Rust、Python、Go、JavaScript、TypeScript/TSX、HTML、CSS、Zig、Bash、Dart、Kotlin、Java、C、C++、C#、PHP、Swift、Ruby、SQL、Dockerfile、YAML、HCL、Vue、Svelte（23 种语法） | ✅ 完成 | `src/extract.rs::Lang`、`Cargo.toml`（23 个 `tree-sitter-*` 依赖）；每个语言族至少一个测试（`rust_extracts_sig_import_call`、`go_extracts_import_signature_and_call`、`typescript_and_tsx_extract_symbols`、`dart_kotlin_java_extract_symbols`、`c_cpp_csharp_extract_symbols`、`php_swift_ruby_extract_symbols`、`sql_docker_yaml_hcl_vue_svelte_extract_structure` 等） |
| 2 | F | `package.json`、其他 manifest/lockfile、`Makefile`、Markdown 安装片段的结构化提取 | ✅ 完成 | `dehydrate_package_json`、`dehydrate_manifest`、`dehydrate_makefile`、`dehydrate_markdown`；测试 `package_json_extracts_lifecycle_scripts`、`makefile_extracts_targets_and_recipe_lines`、`markdown_extracts_dangerous_install_commands_only` |
| 3 | F | 签名/import/调用提取为扁平 `AstSummary` JSON 记录 | ✅ 完成 | `struct AstSummary`、`fn dehydrate` |
| 4 | F | 跨界引用标记 `[EXTERNAL_BLACKBOX]` | ✅ 完成 | `fn is_external`；测试 `intra_crate_rust_imports_are_not_external` 确认不会对 `crate::`/`super::` 误标 |
| 5 | B | 丢弃注释与函数体；脱水后立即 drop AST（从不保留） | ✅ 完成 | 由实现方式保证：`dehydrate()` 只返回扁平摘要；`main.rs` 中任何位置都未保存 `tree_sitter::Tree` |
| 6 | B | 残缺语法不 panic | ✅ 完成 | 测试 `broken_input_no_panic` |
| 7 | G | 百兆仓库：内存稳定、不崩溃 | ⬜ 未完成 | 没有已提交的大仓库/压力测试样本，也没有对应规模的 CI job。`--benchmark` 可以*报告*常驻内存，但 `resident_memory_metric`（`src/main.rs`）仅在 `#[cfg(target_os = "linux")]` 下实现；**macOS 上永远返回 `"unavailable"`**，而 CI 的 `macos-latest` job 从未真正验证过这个指标 |
| 8 | G | `extract.rs` 测试覆盖典型输入与残缺输入 | ✅ 完成 | `extract.rs::tests` 内 17 个测试函数，含畸形输入与未知扩展名场景 |

**阶段结论：🟡 基本完成。** 唯一未验证的门禁是百兆内存稳定性声明；且 macOS（一个受支持的 CI/发布目标）目前完全没有可用的常驻内存指标。

---

## P2 — 模型层（多模型 + 熔断）

ROADMAP 状态：**已完成 ✓**

| # | Type | 条目 | 状态 | 证据 |
|---|---|------|------|------|
| 1 | F | `ModelClient` + `Transport` trait 抽象 | ✅ 完成 | `src/model.rs::ModelClient`、`trait Transport` |
| 2 | F | 带 small/large role 路由的 `Registry` | ✅ 完成 | `struct Registry { small, large }`、`enum Role` |
| 3 | F | 每调用硬超时 | ✅ 完成 | `UreqTransport` 接入 `.timeout(timeout)`；internal-gate PASS「Model transport has a hard timeout」 |
| 4 | F | 连续失败触发熔断 | ✅ 完成 | `struct Breaker`；测试 `timeouts_trip_breaker`、`bad_status_not_retried_exhausts` |
| 5 | F | 指数退避恢复 | ✅ 完成 | `fn backoff`，在 `ModelClient::complete` 中调用 |
| 6 | F | 密钥不入日志，`Debug` 输出脱敏 | ✅ 完成 | `impl fmt::Debug for ModelSpec`；测试 `key_redacted_in_debug` |
| 7 | F | 真实 `[[model]]` TOML 配置解析（role/endpoint/model/key_env/timeout_ms/max_retries） | ✅ 完成 | `FileModelConfig`；测试 `parses_model_blocks`、`rejects_unknown_model_role`、`rejects_wrong_types_inside_model_blocks`、`parses_documented_model_config`、`local_model_can_omit_key_env` |
| 8 | G | 超时/坏响应有模拟测试，熔断确实触发 | ✅ 完成 | `model.rs` 中 `mod tests` 的 `Fake` transport |
| 9 | G | 任何位置都无明文密钥（文档、debug 输出） | ✅ 完成 | internal-gate PASS「Docs avoid direct API key command-line values」；测试 `key_redacted_in_debug` |
| 10 | B | 不缓存、不持久化模型调用 | ✅ 完成 | `Cargo.toml`/`model.rs` 中没有缓存 crate 或磁盘缓存路径 |

**阶段结论：✅ 完成。**

---

## P3 — ReACT 调度器

ROADMAP 状态：**已完成 ✓**

| # | Type | 条目 | 状态 | 证据 |
|---|---|------|------|------|
| 1 | F | 有界状态机（`max_steps`/`max_errors`） | ✅ 完成 | `src/react.rs::ReAct::run` |
| 2 | F | 工具调用协议提示（`<TOOL_CALL>`/`<FINAL>`） | ✅ 完成 | `fn initial_prompt`；测试 `initial_prompt_declares_tool_protocol` |
| 3 | F | `$SEED` 别名解析为完整 seed 文本作为工具输入 | ✅ 完成 | `fn resolve_tool_input`；测试 `seed_alias_feeds_tool_observation` |
| 4 | F | 编译期 `enum` + `match` 技能路由（`coarse_filter`、`converge`） | ✅ 完成 | `src/skills.rs::Skill` |
| 5 | G | 未知技能/坏 JSON 熔断为 `Partial`，绝不 panic | ✅ 完成 | 测试 `unknown_skill_trips_to_partial`、`bad_json_trips_to_partial` |
| 6 | G | 触达步数上限返回 `Partial` 而非无限循环 | ✅ 完成 | 测试 `step_cap_returns_partial_not_hang` |
| 7 | F | 提示词感知报告语言（en/zh） | ✅ 完成 | 测试 `initial_prompt_declares_report_language` |
| 8 | F | 注入 scope 规则，测试/样本永不被报成生产风险 | ✅ 完成 | 测试 `prompts_carry_scope_rubric` |

**阶段结论：✅ 完成。**

---

## P4 — 确定性 Reduce + 报表

ROADMAP 状态：标题未标 ✓；README 自述「进行中」。这是功能增长最多的阶段，下面拆成三组呈现。

### P4a — 确定性账本与 Markdown 报告

| # | Type | 条目 | 状态 | 证据 |
|---|---|------|------|------|
| 1 | F | 确定性 AST 粗筛规则引擎 | ✅ 完成 | `src/report.rs::findings_from_seed`、`push_call_risk`、`push_supply_chain_risks`、`push_manifest_risks`、`push_container_global_risks` |
| 2 | F | 严重度 + 路径 scope 分级（Production/CI/Test/TestFixture/Docs，严重度封顶） | ✅ 完成 | `PathScope::classify`；测试 `path_scope_classifies_common_layouts`、`production_panic_edge_stays_high`、`panic_edge_in_tests_is_capped_to_low`、`fixture_supply_chain_is_capped_to_low` |
| 3 | F | Markdown 账本渲染器，双语标题 | ✅ 完成 | `render_markdown_with_language`、`render_table_with_language`；测试 `renders_localized_markdown` |
| 4 | F | 显式输入覆盖率报告（候选/脱水/seed 字节/上限/批次） | ✅ 完成 | `struct InputCoverage`、`markdown_section`、`agent_gate_coverage` |
| 5 | F | 单条记录截断可见性（原因、原始字节 vs 压缩后字节） | ✅ 完成 | `struct TruncatedRecord`、`compact_seed_record_with_limits`；测试 `compact_seed_record_caps_oversized_files`；internal-gate PASS「Model seed truncation is reported」 |
| 6 | G | 在已知样本上命中预埋风险 | ✅ 完成 | `tests/repo_intake_fixtures.rs`（10 个恶意样本 + 1 个良性样本全部通过） |
| 7 | G | 完整审计 stdout 只含最终报告 | ✅ 完成 | internal-gate PASS「Full audit stdout is reserved for the final report」；测试 `scan_only_stdout_remains_jsonl_not_benchmark_json` |
| 8 | G | 无效配置明确失败，绝不静默回退默认值 | ✅ 完成 | 测试 `dirty_values_reject_config_not_silent_default`、`valid_toml_wrong_types_reject_config_not_silent_default`、`rejects_dirty_env_lines` |
| 9 | G | `--module` 审计限定在项目根内，不串到全局 | ✅ 完成 | 测试 `absolute_module_must_stay_inside_target`、`absolute_module_inside_target_is_allowed`；internal-gate PASS「Module path is contained by project root」 |
| 10 | G | fake-endpoint 完整审计 smoke 证明用户路径可用 | 🟡 部分完成 | 仅有人工证据：`reports/full-audit-local-model-test.md` 是针对某个本地 OpenAI 兼容端点跑出来的。**没有接成自动化/可在 CI 复现的测试**（需要 mock HTTP server 或录制好的 fixture 响应）。且该报告早于当前「small-model Map 默认不激活」的行为，已经不能反映当前默认的纯 Reduce 路径 |

### P4b — Agent gate 与 policy

| # | Type | 条目 | 状态 | 证据 |
|---|---|------|------|------|
| 1 | F | 稳定文本契约（`VERDICT`/`WHY`/`BLOCKERS`/`SAFE_TO_AGENT_RUN`） | ✅ 完成 | `fn render_agent_gate`；`tests/repo_intake_fixtures.rs` |
| 2 | F | 稳定 JSON 契约（`schema_version`、`verdict`、`safe_to_agent_run`、`exit_reason`、`why`、`blockers`、`coverage`、`findings`、`policy_actions`） | ✅ 完成 | `struct AgentGateJson`；测试 `agent_gate_json_exposes_stable_verdict_shape`（黑盒） |
| 3 | F | 仅 `SAFE_TO_AGENT_RUN: yes` 时退出码为 `0`，`CAUTION`/`REJECT`/`INCOMPLETE` 均非零 | ✅ 完成 | `tests/repo_intake_fixtures.rs`（10 个恶意样本均断言非零退出码） |
| 4 | F | 供应链规则集：npm 生命周期脚本、manifest/lockfile 缺口、git/path/http 依赖来源、`build.rs` 命令边界、shell/Dockerfile 下载后执行、base64 解码后执行、GitHub Actions 权限/触发器风险、secrets 与 shell 耦合、未 pin 的 Actions、Docker root/远程仓库模式、可疑二进制/归档 artifact | ✅ 完成 | `tests/fixtures/repo-intake/` 下 21 个样本，由 `sift eval-corpus`（`eval_cases`，21 例）与 `tests/repo_intake_fixtures.rs` 共同验证 |
| 5 | F | 项目本地 `sift-policy.toml`（`max_candidate_files`、`[[allowlist]]`、`[[denylist]]`、`[[severity_override]]`） | ✅ 完成 | `config.rs` 中 `load_policy_config`/`parse_policy_config`；测试 `parses_policy_schema_and_rejects_bad_severity`；`report.rs` 中 `apply_policy`/`policy_match`/`policy_override_match` |
| 6 | F | 可疑二进制/归档 artifact 清单 | ✅ 完成 | `inspect_suspicious_artifact`、`is_binary_or_archive_name`；样本 `binary-artifact-exec`、`binary-extension`、`archive-payload` |
| 7 | F | `sift eval-corpus`：≥20 例精度表 | ✅ 完成 | `run_eval_corpus`，21 个 `eval_cases`；测试 `eval_corpus_reports_twenty_or_more_cases` |
| 8 | G | 近期回归修复：Cargo.lock 的 registry 来源不再被误判成 git dependency；`workflow-write-all` 不再把单项 `contents:`/`actions:`/`packages: write` 和真正的 broad write-all 混为一谈；`record_truncated > 0` 本身不再直接判 `INCOMPLETE`；VCS 元数据目录（`.git`、`.hg`、`.svn`、`.jj`）默认从扫描中排除 | ✅ 完成 | 已落地在当前 HEAD `f9a374b`，覆盖了 `reports/project-audit-2026-07-01.md`（针对父提交 `88c5334` 写成）中列出的待办项。证据：测试 `ignores_cargo_lock_crates_io_registry_source`、`flags_broad_but_not_scoped_workflow_write_permissions`；`scanner.rs::VCS_METADATA_DIRS`；`report.rs::gate_incomplete_reasons` 已不再读取 `record_truncated` |
| 9 | ⛑ | **本会话已修复的 dogfood 发现：** `sift . --agent-gate` 审计 sift 自身仓库时曾返回 `CAUTION`，根因是两个真实 bug，现均已修复——详见[自我审计 dogfood 检查](#自我审计-dogfood-检查) | ✅ 完成 | (a) 在 `report.rs`/`extract.rs` 中新增 `looks_like_eval_invocation`，要求独立的 `eval` 单词后面跟一个 shell 替换词归才算命中，让 "eval corpus" 这类英文行文不再误触 `dynamic-shell-eval`；测试 `flags_real_dynamic_shell_eval_invocation`、`ignores_eval_used_as_an_english_word`、`markdown_prose_mentioning_eval_corpus_is_not_a_command`。(b) 把 `[[allowlist]]` policy 匹配从只适用于 `RiskFinding` 扩展到也适用于 `coverage.suspicious_artifacts`（新增 `apply_policy_to_artifacts`/`policy_match_artifact`），并新增了一个真正的根目录 `sift-policy.toml`，为 `.githooks/pre-commit` 和 `tests/fixtures/repo-intake/` 下的合成 artifact 加白；测试 `policy_allowlist_suppresses_matching_suspicious_artifact`、`policy_allowlisting_every_artifact_reaches_accept`、`policy_allowlist_matches_one_tag_within_a_combined_artifact_reason`。两项修复后重跑：blocker 归零，但 verdict 仍为 `CAUTION`——现已确认这是预期中的正确结果，不是 bug（详见 dogfood 部分） |

### P4c — 运行模式

| # | Type | 条目 | 状态 | 证据 |
|---|---|------|------|------|
| 1 | F | `--benchmark` 本地 telemetry（不调用模型；可选 USD 成本估算） | ✅ 完成 | `tests/benchmark_mode.rs`（3/3 通过） |
| 2 | F | `sift github owner/repo` 安全 intake——绝不 build/install/跑 hook/碰 submodule；扫描前检查文件/字节上限、`.gitmodules`、Git LFS | ✅ 完成 | `run_github_intake`、`parse_github_repo`、`inspect_checkout_dir`；测试 `github_repo_parser_accepts_owner_repo_and_https`、`checkout_inspection_reports_lfs_and_limits`、`github_intake_rejects_non_github_url_without_network`（黑盒）。`git` fetch 与递归调用本地 `sift` 均跑在 `run_command_with_timeout` 之下（120s / 600s 硬 deadline，超时即 kill） |
| 3 | F | `sift doctor`——配置/密钥/端点诊断 | 🟡 部分完成 | 已实现（`run_doctor`、`check_config_permissions`、`check_file_config`、`check_endpoint_key_pair` 等），但**自动化测试覆盖为零**——`config.rs::tests` 里没有任何单测覆盖 `run_doctor`/`Doctor`，`tests/` 下也没有黑盒测试拉起 `sift doctor`。内部门禁「每个文件有 `#[cfg(test)]`」的 BT 检查之所以对 `config.rs` 显示 PASS，只是因为同一文件里*其他*函数有测试——它看不见这个缺口 |
| 4 | F | `--save`/`--save-to` 持久化报告（`reports/sift-audit-result-YYYYMMDD-NNN.md`） | ✅ 完成 | `main.rs` 中 `save_audit_result`、`next_audit_result_path`、`utc_yyyymmdd`、`civil_from_days` |
| 5 | F | `--report-language {en,zh}` 双语 Markdown 报告 | ✅ 完成 | `ReportLanguage`；测试 `localized_headings_render_for_zh` |
| 6 | F | `--debug` 额外 stderr 诊断 | ✅ 完成 | `main.rs` 中的 debug `eprintln!` 代码块 |
| 7 | B | 小模型 Map（`map_small_pool`）保留为未激活的诊断脚手架，默认完整审计路径不调用 | 🟡 部分完成（按设计如此） | `model.rs` 中代码与 4 个测试均存在（`small_pool_maps_successful_observations` 等），但 `main.rs` 只打印 `"small-model Map inactive: reduce converges from deterministic findings"`，从不调用它。这与 AGENT.md 的表述完全一致——它被正确标注成脚手架，不是缺陷——但仍是一个**尚未决定的路线图问题**：是在行为级门禁后重新接入，还是彻底下线 |

**阶段结论：🟡 基本完成——与项目自述的「P4 进行中」一致。** 真正悬而未决的工程问题是 P4a 的 #10（没有 CI 自动化的完整审计 smoke）和 P4c 的 #3（`doctor` 无测试）；小模型 Map 的去留（P4c #7）是一个明确的待决策问题，不是 bug。

---

## P5 — 内部质量门禁

ROADMAP 状态：标题已在本会话中补上 ✓，功能与门禁本身早已完整落地且全绿。

| # | Type | 条目 | 状态 | 证据 |
|---|---|------|------|------|
| 1 | F | `audit.rs` 自审模块，覆盖 CQ/SEC/RB/DF/BT/CC/UX 维度评分 | ✅ 完成 | `src/audit.rs::run_checks`（13 项检查） |
| 2 | F | 把维护者专用报告写入 `reports/internal-gate.md`（已 gitignore） | ✅ 完成 | `write_internal_gate`；`.gitignore` 含 `/reports/` |
| 3 | F | 对公开 CLI 隐藏（由 `SIFT_INTERNAL_GATE=1` 触发，不是文档化 flag） | ✅ 完成 | `main.rs` 中 `internal_gate_target()`；测试 `self_audit_flag_is_not_public_cli_argument` 确认不存在 `--self-audit` flag |
| 4 | F | 接入 `make internal-gate` / `make ci` | ✅ 完成 | `Makefile`；本次会话验证（`make ci` 退出码 0） |
| 5 | G | 硬规则无 FAIL/WARN，包括无 broad `dead_code` allow、无原始中文源码字符串、报告流边界干净、seed 截断可见 | ✅ 完成 | 本次会话现跑结果：**13/13 PASS，0 WARN，0 FAIL**（`reports/internal-gate.md`） |
| 6 | ⚑ | 测试覆盖检查（`BT`）只到文件粒度 | 🟡 已知局限 | `test_coverage_status` 只检查文件里*某处*是否含 `#[cfg(test)]`——无法探测某个具体函数（例如 `run_doctor`）在一个整体有测试的文件里其实完全没被测。见 P4c #3 |

**阶段结论：✅ 完成。** `ROADMAP.md`/`ROADMAP.zh.md` 的 P5 标题已在本会话中改为「— done ✓ / 已完成 ✓」以匹配现状，剩余建议是把 `BT` 检查收紧到函数级粒度。

---

## P6 — 发布加固

ROADMAP 状态：标题未标 ✓，但已有相当充分的证据。

| # | Type | 条目 | 状态 | 证据 |
|---|---|------|------|------|
| 1 | F | 体积调优的 release profile（`opt-level=z`、`lto`、`codegen-units=1`、`strip`、`panic=abort`） | ✅ 完成 | `Cargo.toml::[profile.release]` |
| 2 | F | Makefile 安装/卸载路径（默认 `~/.local/bin`，可用 `PREFIX`/`BINDIR` 覆盖） | ✅ 完成 | `Makefile` 的 `install`/`uninstall` target |
| 3 | F | Git hooks 安装/卸载；pre-commit 跑 `make local-ci` | ✅ 完成 | `Makefile` 的 `githooks-install`/`githooks-uninstall`；`.githooks/pre-commit` |
| 4 | F | CI：在 `ubuntu-latest` + `macos-latest` 矩阵上跑 fmt/test/clippy/internal-gate | ✅ 完成 | `.github/workflows/ci.yml` |
| 5 | F | Release workflow：SemVer 标签校验、macOS amd64/arm64 构建、`tar.xz` + `sha256`、environment 审批后 draft→published | ✅ 完成 | `.github/workflows/release.yml`；已有标签 `v0.1.0`、`v0.2.0` |
| 6 | F | Homebrew tap 自动发布（渲染并推送 `jamiesun/homebrew-tap` formula） | ✅ 完成 | `release.yml::homebrew` job；依赖仓库 secret `HOMEBREW_TAP_TOKEN` 是否配置，这一点超出本仓库自身可验证的范围 |
| 7 | F | 更多语法 | ⏳ 待定（开放式） | 已交付 23 种 tree-sitter 语法 + 4 种结构化提取器（见 P1）；ROADMAP 有意将其保持无上限，因此永远无法标记为「完全完成」 |
| 8 | F | `--benchmark`、`--agent-gate --format json`、`eval-corpus` 的稳定 JSON 输出契约（`schema_version`） | ✅ 完成 | `benchmark_mode_outputs_stable_json_without_model_keys`、`agent_gate_json_exposes_stable_verdict_shape` 均断言 `schema_version: 1` |
| 9 | G | 单文件分发 | ✅ 完成 | `release.yml` 把单个 `sift` 二进制（+ docs/README/config 模板）打进一个 `tar.xz` |
| 10 | G | 内部门禁通过 | ✅ 完成 | 见 P5 |
| 11 | G | 文档 ↔ 功能一致 | 🟡 部分完成（仅人工） | 没有任何自动化检查会把文档（支持语言列表、CLI flag、版本号）与源码事实来源做 diff；本次会话通过人工交叉阅读验证，但**`make ci` 无法捕捉未来的漂移** |
| 12 | G | `brew install jamiesun/tap/sift` 由 release checksum 支撑 | ✅ 完成（未做外部复核） | `release.yml` 中已有 `sha256`/formula 渲染逻辑；本次会话未对真实的 `jamiesun/homebrew-tap` 仓库做独立复核 |

**阶段结论：🟡 基本完成。** 两条悬而未决的线：文档↔代码一致性没有自动化守卫；「更多语法」是有意保持无上限的目标，而不是一个可以关闭的门禁。

---

## 横切检查：工程契约（ROADMAP.md）

| # | 规则 | 状态 | 证据 |
|---|------|------|------|
| 1 | 标记完成的阶段有行为级证据，不只是类型层接线 | ✅ P0–P3 成立；🟡 上文标注了两处例外（P0 #7、P4c #3） |
| 2 | 完整审计 stdout 是最终报告；`--scan-only` 是 JSONL；诊断信息不进 stdout | ✅ 完成 | 见 P4a #7 |
| 3 | 报告披露扫描/脱水/送入模型/跳过/截断的规模 | ✅ 完成 | `InputCoverage`、`AgentGateCoverage` |
| 4 | 用户配置缺失时从安全默认值自动创建；配置文件存在但无效时必须失败，不能回退默认值 | ✅ 完成 | 见 P4a #8 |
| 5 | `src/` 内运行时文本、prompt、注释只用英文 | ✅ 完成 | internal-gate PASS「Program source avoids raw CJK literals」 |

## 横切检查：完成的样子（ROADMAP.md）

| # | 标准 | 状态 |
|---|------|------|
| 1 | 零配置可跑；自动创建 `~/.sift/config.toml`；缺 Key 即退给提示；不挂起 | ✅ 完成 |
| 2 | 百兆仓库内存稳定；坏输入不崩溃 | ⬜ 未完成——见 P1 #7 |
| 3 | 报表定位行号 + 跨模块依赖 + 并发/资源风险 | ✅ 完成 |
| 4 | 报表声明输入覆盖率和截断状态；覆盖不完整时绝不能看起来像完整结论 | ✅ 完成 |
| 5 | 任一外部调用必超时；失败即熔断出半成品，绝不死磕 | ✅ 完成——模型 HTTP 调用（`model.rs`）与 GitHub intake 子进程（`run_command_with_timeout`，120s/600s）均已验证 |
| 6 | 同一二进制审项目与 `--module` 不串 | ✅ 完成 |
| 7 | 内部发布门禁无 FAIL，硬规则无 WARN | ✅ 完成 |

---

## 非目标护栏

确认 ROADMAP.md 里「绝不做」的铁律没有被越界。

| # | 非目标 | 是否守住 | 证据 |
|---|--------|----------|------|
| 1 | 不做向量库/embedding/RAG | ✅ 守住 | `Cargo.toml` 依赖列表中没有向量库/embedding crate |
| 2 | 不做运行时插件/动态技能注册 | ✅ 守住 | `skills.rs::Skill` 是编译期 `enum` + `match`；无动态加载类依赖 |
| 3 | 不做服务化/Web UI/多租户 | ✅ 守住 | `Cargo.toml` 中无 web-server crate；只通过 `clap` 提供 CLI |
| 4 | 不允许 panic 主进程 | ✅ 守住（启发式，非形式化证明） | internal-gate 对显式 `panic!` 和 `unwrap()`/`expect()` 字面模式检查均 PASS。注意：release profile 里的 `panic = "abort"` 只是改变了*一旦真的 panic*时的 unwind 行为，本身并不是「不会 panic」的保证；真正的保证来自源码文本扫描，它无法捕捉例如下标越界/溢出类 panic |
| 5 | 不允许无超时阻塞 | ✅ 守住 | 模型调用：`model.rs` 中的 `ureq` timeout；子进程：`run_command_with_timeout`（git fetch 120s，递归调用本地 `sift` 600s，超时即 kill） |
| 6 | 模块审计不能膨胀成全局 | ✅ 守住 | 见 P4a #9 |
| 7 | 不靠「直接试用」替代审计 | ✅ 守住 | `sift github` 无论 flag 如何都绝不 build/install/跑 hook/碰 submodule；`GithubCli` 上的 `--no-build`/`--no-install` 是明确的安全意图标记，不是开关——工具本来就两种情况下都不会 build 或 install |
| 8 | 脚手架不得冒充产品能力 | ✅ 守住 | 小模型 Map 在代码输出和文档中都被明确标成「未激活的诊断脚手架」，不计入已交付的默认行为 |
| 9 | 不允许静默回退 | ✅ 守住 | 见 P4a #8；无效配置总是明确失败 |

---

## 自我审计 dogfood 检查

AGENT.md 写道「sift 必须通过内部发布门禁」。这句话其实涉及**两个不同的门禁**，本清单刻意把它们分开：

1. **内部质量门禁**（`SIFT_INTERNAL_GATE=1`，即 `make internal-gate`）——sift 自身的*代码质量*门禁。**结果：13/13 PASS，0 FAIL，0 WARN。** ✅ 这正是 ROADMAP.md 和 AGENT.md 所说的门禁，且是全绿的。
2. **Agent gate**（`sift . --agent-gate`）——用来在 agent 执行 setup/build/install 之前，筛查任意第三方仓库的*产品功能*。ROADMAP 并没有要求 sift 用这个门禁审自己的仓库必须得到 ACCEPT，但拿它做一次 dogfood 检查很有意义。

### 第一次跑分（本会话开始时）：发现两个真实 bug

```text
VERDICT: CAUTION
SAFE_TO_AGENT_RUN: no
coverage: candidate_files=69 dehydrated_files=62 unsupported_files=7
          record_truncated=12 seed_bytes=148402
```

- **规则误报：** `docs/ROADMAP.zh.md` 等行文文件被标成 `dynamic-shell-eval`（scope=docs，MEDIUM），原因纯粹是英文短语 **"eval corpus"**（正是 sift 自己的 `eval-corpus` 功能名）包含子串 `"eval "`，而 `looks_like_dynamic_shell_eval`（`src/report.rs`）和 `looks_like_shell_command`（`src/extract.rs`）对这个子串都是无条件匹配。这不是真正的 shell-eval 风险。
- **未加白但合法的 artifact：** `.githooks/pre-commit`（一个没有扩展名、真实存在且已提交的可执行文件）和两个已提交的测试样本（`archive-payload/assets/payload.tar.gz`、`binary-extension/bin/tool.dylib`）触发了可疑 artifact 规则。项目根目录没有真正的 `sift-policy.toml`（只有 `sift-policy.example.toml`），而即使有，policy 的 `[[allowlist]]` 匹配也只适用于 `RiskFinding`，从未覆盖过 `coverage.suspicious_artifacts`——所以这些 blocker 根本无法被压制。

### 本会话落地的修复

1. 在 `report.rs` 和 `extract.rs` 中都新增了 `looks_like_eval_invocation`（单词边界 + shell 替换词 token 检查），让 `eval` 只在这个独立单词后面紧跟 command substitution、反引号或 `$变量` 时才命中，永远不会误读提到 "eval corpus"/"retrieval" 的英中文行文。测试：`flags_real_dynamic_shell_eval_invocation`、`ignores_eval_used_as_an_english_word`、`markdown_prose_mentioning_eval_corpus_is_not_a_command`。
2. 把 policy 引擎扩展为 `[[allowlist]]` 也能压制 `suspicious_artifacts` blocker，用 `rule` 匹配 artifact 的 `reason` 标签（`apply_policy_to_artifacts`、`policy_match_artifact`，均在 `report.rs`；也处理了逗号拼接的多 reason 情况）。测试：`policy_allowlist_suppresses_matching_suspicious_artifact`、`policy_allowlisting_every_artifact_reaches_accept`、`policy_allowlist_matches_one_tag_within_a_combined_artifact_reason`。
3. 新增了一个真正的根目录 `sift-policy.toml`（之前只有 `sift-policy.example.toml`），为 `.githooks/pre-commit` 和 `tests/fixtures/repo-intake/` 下的合成 artifact 加白，每条都写了理由。`sift-policy.example.toml` 也同步补充了 artifact 加白写法的文档示例。

### 第二次跑分（修复后）：blocker 消失，但 verdict 仍然（正确地）为 CAUTION

```text
VERDICT: CAUTION
SAFE_TO_AGENT_RUN: no
coverage: candidate_files=72 dehydrated_files=64 unsupported_files=8
          record_truncated=12 seed_bytes=148542
BLOCKERS: none
POLICY:
- suppressed artifact extensionless_or_binary_executable at .githooks/pre-commit by allowlist (...)
- suppressed artifact binary_or_archive_extension at tests/fixtures/repo-intake/archive-payload/assets/payload.tar.gz by allowlist (...)
- suppressed artifact binary_or_archive_extension at tests/fixtures/repo-intake/binary-extension/bin/tool.dylib by allowlist (...)
```

（给未来编辑本节的人一个提醒：如果把这两条规则的触发形状写得过于具体、可直接复现，就会让这个文件自己触发它们。请让任何这类示例都保持适度改写。）

两个根因都已修复并验证：eval 误报消失了（唯一剩下的一条 `dynamic-shell-eval` 是一个真实的 shell 调用样本——`bash` 内联 `-c` 命令并插值了一个 secret，正是预期中应该被标出的），且三个未加白 artifact blocker 现在都已被写有理由的白名单压制。

**但 verdict 仍然是 `CAUTION`，现在已确认这是预期中的正确结果——不是需要追逐消除的缺陷。** 剩余的 40 条发现全部是 `Severity::Low`，没有 `Medium`/`High`，且每一条都能追溯到下面两个有意设计的来源之一：

- `tests/fixtures/repo-intake/` 下 21 个合成攻击模式样本（与 `sift eval-corpus` 评分用的是同一份语料）。它们的存在就是为了证明供应链规则引擎能检测到 `npm-lifecycle-script`、`download-execute`、`dependency-git-source`、`workflow-write-all` 等。如果自扫让这些发现消失，那说明规则坏了，而不是修好了。
- `tests/*.rs` 里的 `panic-edge`（`.expect()`/`.unwrap()`）发现。铁律 #1 只禁止在 `src/` 里用 `unwrap()`/`expect()`，在测试里用它们完全正常且正确，`PathScope::classify` 也已经把这些封顶到 Low——它们依旧会以发现形式出现（信息性的），只是不能被静静藏起来。

Agent gate 的 verdict 规则（`render_agent_gate`）只有在 `findings` 完全为空时才返回 `ACCEPT`。强行让 sift 自己的仓库做到这一点，只能靠删除自己的回归语料，或者对 `tests/` 下所有规则一概加白，这两种做法都会抹掉本清单 P4a/P4b 行引用的证据。因此诚实、经得起推敲的 dogfood 结论应该是：**0 条 High 发现、0 条无法解释的 blocker、每一条 Low 发现都有归属**——而不是字面上的 `ACCEPT`。

---

## 汇总：待办事项

把上文所有非 ✅ 完成的条目汇总在一处。上一份快照中的两项已在本会话中解决，此处不再列入（agent gate 自审 CAUTION 的根因已修复；ROADMAP P5 标题已刷新）——前者详见[自我审计 dogfood 检查](#自我审计-dogfood-检查)。

| 事项 | 阶段 | 状态 | 建议下一步 |
|------|------|------|------------|
| 没有黑盒测试断言「完整审计缺 Key 时退出码为 1」 | P0 | 🟡 部分完成 | 在 `tests/` 下新增一个集成测试 |
| 没有百兆压力测试样本；macOS 上常驻内存指标永远是 `"unavailable"` | P1 | ⬜ 未完成 | 新增大仓库 smoke 测试；把 `resident_memory_metric` 扩展到 macOS（`task_info`/`ps`） |
| fake-endpoint 完整审计 smoke 仅为人工验证，未接入 CI，且早于当前小模型 Map 默认不激活的行为 | P4a | 🟡 部分完成 | 新增一个基于 mock HTTP server、端到端跑通 `react::ReAct` 的集成测试 |
| 原有的 policy 压制逻辑（针对 `RiskFinding` 的 `apply_policy`/`policy_match`/`policy_override_match`）没有直接的端到端单测验证压制本身——只测试了 TOML 解析（`parses_policy_schema_and_rejects_bad_severity`）。本会话新增的 artifact 加白路径有测试，但原有的 finding 加白路径仍然没有 | P4b | 🟡 部分完成 | 在 `report.rs` 中为 `apply_policy`/denylist/severity-override 新增单测，参照新增的 `policy_allowlist_*` artifact 测试写法 |
| `sift doctor` 自动化测试覆盖为零 | P4c | 🟡 部分完成 | 为 `Doctor`/`run_doctor` 补单测，和/或新增 `tests/doctor.rs` 黑盒测试 |
| 小模型 Map 是未激活脚手架；重新接入还是下线仍未决定 | P4c | 🟡 部分完成（按设计如此） | 由维护者决策，之后要么接到行为级门禁之后，要么删除 |
| 「更多语法」没有固定目标 | P6 | ⏳ 待定 | 不算缺陷；按语言诉求逐条建 issue 跟踪，而不是靠本清单 |
| 文档 ↔ 代码一致性没有自动化守卫 | P6 | 🟡 部分完成 | 可以考虑在 `audit.rs` 里加一条检查，把 `README.md` 的支持语言列表和 `extract.rs::Lang` 的变体做交叉核对 |

---

## 如何刷新本快照

```sh
cargo build
make ci                                   # fmt-check + test + clippy -D warnings + internal-gate
cat reports/internal-gate.md              # P5 门禁细节（已 gitignore，仅本地）
cargo run --quiet -- . --agent-gate --format json   # 现跑一次自我扫描（对应上文 dogfood 检查）
sift eval-corpus                          # repo-intake 精度表
```

本文件反映的是某一个提交时间点的状态。每当某个阶段的证据发生变化，请重新执行上面的命令，
并更新「快照信息」表、各阶段表格与「汇总：待办事项」——不要在没有重新核对证据的情况下手改状态标记。
