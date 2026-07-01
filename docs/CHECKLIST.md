# sift Acceptance Checklist

> English | [中文](CHECKLIST.zh.md)
>
> A point-in-time acceptance snapshot of every feature and gate promised in
> [ROADMAP.md](ROADMAP.md), scoped strictly to its phases (P0–P6) and its
> non-goals. Nothing outside that boundary is graded here — see
> [AGENT.md](../AGENT.md) for the hard rules this checklist assumes.
> Line numbers are as of the snapshot commit below and may drift; prefer the
> named function/test when they disagree.

## Snapshot

| | |
|---|---|
| Commit | `f9a374b` — "fix agent gate issue regressions (#39)" (4 commits past tag `v0.2.0`), plus uncommitted follow-up fixes from this session (see [Self-audit dogfood check](#self-audit-dogfood-check)) |
| Date assessed | 2026-07-01 |
| `cargo build` | ✅ pass |
| `make ci` (`fmt-check` + `test` + `clippy -D warnings` + `internal-gate`) | ✅ pass, exit 0 |
| Tests | ✅ 127 passed, 0 failed (119 unit tests in `src/**` + 8 black-box tests in `tests/*.rs`) |
| Internal quality gate (`reports/internal-gate.md`) | ✅ 13/13 checks PASS, 0 WARN, 0 FAIL |

## Legend

| Mark | Meaning |
|---|---|
| ✅ **Done** | Shipped with behavior-level evidence (a passing test and/or a real run performed for this snapshot) — not only type-level plumbing or a happy-path unit test. |
| 🟡 **Partial** | Shipped but capped in scope, intentionally inactive scaffolding, or missing one specific proof point noted in the row. |
| ⏳ **Pending** | No fixed target yet, awaiting a maintainer decision, or explicitly open-ended in `ROADMAP.md`. |
| ⬜ **Not done** | Promised but not implemented, or no automated evidence exists at all. |

Mapped to the three buckets this checklist is meant to answer: **完成 = ✅**, **待定 = ⏳**, **未完成 = 🟡 / ⬜**.

`Type` column: **F** = Feature bullet, **G** = Gate/acceptance-criterion bullet, **B** = Boundary constraint, taken verbatim from each phase's ROADMAP.md text.

---

## P0 — Scaffold

ROADMAP status: **done ✓**

| # | Type | Item | Status | Evidence |
|---|---|------|--------|----------|
| 1 | F | Fallback key resolution: CLI key file › ENV › project `.env` › `~/.sift/config.toml` › default | ✅ Done | `src/config.rs::Config::resolve`; tests `parses_project_env_file`, `explicit_api_key_file_must_be_readable_and_non_empty` |
| 2 | F | Bounded-channel scanner (walk → bounded channel, consume & drop) | ✅ Done | `src/scanner.rs` (`crossbeam_channel::bounded::<PathBuf>(1024)`); test `scan_skips_ignored_dirs_and_large_files` |
| 3 | F | Minimal end-to-end wiring: parse → Config → schedule → report → exit code | ✅ Done | `src/main.rs::main` |
| 4 | G | `cargo build` green | ✅ Done | Verified this session (`make ci` exit 0) |
| 5 | G | Zero `unwrap()`/`expect()` in `src/` | ✅ Done | `reports/internal-gate.md`: "No direct unwrap/expect in src" — PASS |
| 6 | G | `--scan-only` scans without any model key | ✅ Done | `tests/benchmark_mode.rs::scan_only_stdout_remains_jsonl_not_benchmark_json` |
| 7 | G | Missing large-model key exits before scheduling a full audit | 🟡 Partial | Code path exists (`src/main.rs:83-86`, `config::missing_large_key_hint`), unit-tested for message content only (`missing_key_hint_uses_parseable_model_block`); **no black-box test spawns the real binary with no key on a non-`--scan-only`/`--agent-gate`/`--benchmark` path to assert the process exit code** |

**Phase verdict: ✅ Done**, with one test-coverage gap (#7).

---

## P1 — Tier-0 AST dehydrate

ROADMAP status: **done ✓**

| # | Type | Item | Status | Evidence |
|---|---|------|--------|----------|
| 1 | F | tree-sitter grammar coverage: Rust, Python, Go, JavaScript, TypeScript/TSX, HTML, CSS, Zig, Bash, Dart, Kotlin, Java, C, C++, C#, PHP, Swift, Ruby, SQL, Dockerfile, YAML, HCL, Vue, Svelte (23 grammars) | ✅ Done | `src/extract.rs::Lang`, `Cargo.toml` (23 `tree-sitter-*` deps); one test per language family (`rust_extracts_sig_import_call`, `go_extracts_import_signature_and_call`, `typescript_and_tsx_extract_symbols`, `dart_kotlin_java_extract_symbols`, `c_cpp_csharp_extract_symbols`, `php_swift_ruby_extract_symbols`, `sql_docker_yaml_hcl_vue_svelte_extract_structure`, …) |
| 2 | F | Structural extraction for `package.json`, other manifests/lockfiles, `Makefile`, Markdown install snippets | ✅ Done | `dehydrate_package_json`, `dehydrate_manifest`, `dehydrate_makefile`, `dehydrate_markdown`; tests `package_json_extracts_lifecycle_scripts`, `makefile_extracts_targets_and_recipe_lines`, `markdown_extracts_dangerous_install_commands_only` |
| 3 | F | Extract signatures/imports/calls into a flat `AstSummary` JSON record | ✅ Done | `struct AstSummary`, `fn dehydrate` |
| 4 | F | Cross-boundary references marked `[EXTERNAL_BLACKBOX]` | ✅ Done | `fn is_external`; test `intra_crate_rust_imports_are_not_external` confirms it does **not** over-flag `crate::`/`super::` |
| 5 | B | Bodies/comments omitted; AST dropped immediately after dehydration (never retained) | ✅ Done | By construction: `dehydrate()` returns only the flat summary; no `tree_sitter::Tree` is stored anywhere in `main.rs` |
| 6 | B | Malformed syntax tolerated without panicking | ✅ Done | Test `broken_input_no_panic` |
| 7 | G | 100 MB repo: stable memory, no crash | ⬜ Not done | No committed large-repo/stress fixture or CI job of this scale exists. `--benchmark` can *report* resident memory, but only on Linux (`resident_memory_metric` in `src/main.rs` is `#[cfg(target_os = "linux")]`); **on macOS it always reports `"unavailable"`**, and CI's `macos-latest` job never exercises this metric |
| 8 | G | `extract.rs` tests cover typical + broken input | ✅ Done | 17 test functions in `extract.rs::tests`, including malformed-input and unknown-extension cases |

**Phase verdict: 🟡 Mostly done.** The only unverified gate is the 100 MB memory-stability claim, and macOS (a supported CI/release target) currently has no working resident-memory metric at all.

---

## P2 — Model layer (multi-model + breaker)

ROADMAP status: **done ✓**

| # | Type | Item | Status | Evidence |
|---|---|------|--------|----------|
| 1 | F | `ModelClient` + `Transport` trait abstraction | ✅ Done | `src/model.rs::ModelClient`, `trait Transport` |
| 2 | F | `Registry` with small/large role routing | ✅ Done | `struct Registry { small, large }`, `enum Role` |
| 3 | F | Per-call hard timeout | ✅ Done | `UreqTransport` wires `.timeout(timeout)`; internal-gate PASS "Model transport has a hard timeout" |
| 4 | F | Breaker on consecutive failures | ✅ Done | `struct Breaker`; tests `timeouts_trip_breaker`, `bad_status_not_retried_exhausts` |
| 5 | F | Exponential backoff recovery | ✅ Done | `fn backoff`, used from `ModelClient::complete` |
| 6 | F | Keys never logged; redacted in `Debug` | ✅ Done | `impl fmt::Debug for ModelSpec`; test `key_redacted_in_debug` |
| 7 | F | Real `[[model]]` TOML config parsing (role/endpoint/model/key_env/timeout_ms/max_retries) | ✅ Done | `FileModelConfig`; tests `parses_model_blocks`, `rejects_unknown_model_role`, `rejects_wrong_types_inside_model_blocks`, `parses_documented_model_config`, `local_model_can_omit_key_env` |
| 8 | G | Timeout/bad-response simulated, breaker trips | ✅ Done | `mod tests` `Fake` transport in `model.rs` |
| 9 | G | No plaintext keys anywhere (docs, debug output) | ✅ Done | internal-gate PASS "Docs avoid direct API key command-line values"; test `key_redacted_in_debug` |
| 10 | B | No cache/persistence of model calls | ✅ Done | No cache crate or on-disk cache path in `Cargo.toml` / `model.rs` |

**Phase verdict: ✅ Done.**

---

## P3 — ReACT scheduler

ROADMAP status: **done ✓**

| # | Type | Item | Status | Evidence |
|---|---|------|--------|----------|
| 1 | F | Bounded state machine (`max_steps`/`max_errors`) | ✅ Done | `src/react.rs::ReAct::run` |
| 2 | F | Tool-call protocol prompt (`<TOOL_CALL>`/`<FINAL>`) | ✅ Done | `fn initial_prompt`; test `initial_prompt_declares_tool_protocol` |
| 3 | F | `$SEED` alias resolves to the full seed text for tool input | ✅ Done | `fn resolve_tool_input`; test `seed_alias_feeds_tool_observation` |
| 4 | F | Compile-time skill routing via `enum` + `match` (`coarse_filter`, `converge`) | ✅ Done | `src/skills.rs::Skill` |
| 5 | G | Unknown skill / bad JSON trips to `Partial`, never panics | ✅ Done | Tests `unknown_skill_trips_to_partial`, `bad_json_trips_to_partial` |
| 6 | G | Step cap returns `Partial` instead of looping forever | ✅ Done | Test `step_cap_returns_partial_not_hang` |
| 7 | F | Report-language-aware prompts (en/zh) | ✅ Done | Test `initial_prompt_declares_report_language` |
| 8 | F | Scope rubric injected so tests/fixtures are never reported as production risk | ✅ Done | Test `prompts_carry_scope_rubric` |

**Phase verdict: ✅ Done.**

---

## P4 — Deterministic Reduce + report

ROADMAP status: **not marked done**; README self-reports "in progress." This is the phase carrying the most feature growth, so it is split into three groups below.

### P4a — Deterministic ledger & Markdown report

| # | Type | Item | Status | Evidence |
|---|---|------|--------|----------|
| 1 | F | Deterministic AST coarse-filter rule engine | ✅ Done | `src/report.rs::findings_from_seed`, `push_call_risk`, `push_supply_chain_risks`, `push_manifest_risks`, `push_container_global_risks` |
| 2 | F | Severity + path-scope classification (Production/CI/Test/TestFixture/Docs, severity caps) | ✅ Done | `PathScope::classify`; tests `path_scope_classifies_common_layouts`, `production_panic_edge_stays_high`, `panic_edge_in_tests_is_capped_to_low`, `fixture_supply_chain_is_capped_to_low` |
| 3 | F | Markdown ledger renderer, bilingual headings | ✅ Done | `render_markdown_with_language`, `render_table_with_language`; test `renders_localized_markdown` |
| 4 | F | Explicit input-coverage reporting (candidate/dehydrated/seed bytes/cap/batches) | ✅ Done | `struct InputCoverage`, `markdown_section`, `agent_gate_coverage` |
| 5 | F | Per-record truncation visibility (reason, original vs. compacted bytes) | ✅ Done | `struct TruncatedRecord`, `compact_seed_record_with_limits`; test `compact_seed_record_caps_oversized_files`; internal-gate PASS "Model seed truncation is reported" |
| 6 | G | Hits seeded risks in known fixtures | ✅ Done | `tests/repo_intake_fixtures.rs` (10 malicious + 1 benign fixture, all pass) |
| 7 | G | Full-audit stdout contains only the final report | ✅ Done | internal-gate PASS "Full audit stdout is reserved for the final report"; test `scan_only_stdout_remains_jsonl_not_benchmark_json` |
| 8 | G | Invalid config fails loudly, never silently reverts to defaults | ✅ Done | Tests `dirty_values_reject_config_not_silent_default`, `valid_toml_wrong_types_reject_config_not_silent_default`, `rejects_dirty_env_lines` |
| 9 | G | `--module` audit is contained inside the project root, never bleeds to global | ✅ Done | Tests `absolute_module_must_stay_inside_target`, `absolute_module_inside_target_is_allowed`; internal-gate PASS "Module path is contained by project root" |
| 10 | G | Fake-endpoint full-audit smoke proves the user-facing path | 🟡 Partial | Manual evidence only: `reports/full-audit-local-model-test.md` was produced against a local OpenAI-compatible endpoint. **Not wired as an automated/CI-reproducible test** (needs a mock HTTP server or recorded fixture responses). That report also predates the current "small-model Map inactive by default" behavior, so it no longer reflects the default Reduce-only path |

### P4b — Agent gate & policy

| # | Type | Item | Status | Evidence |
|---|---|------|--------|----------|
| 1 | F | Stable text contract (`VERDICT`/`WHY`/`BLOCKERS`/`SAFE_TO_AGENT_RUN`) | ✅ Done | `fn render_agent_gate`; `tests/repo_intake_fixtures.rs` |
| 2 | F | Stable JSON contract (`schema_version`, `verdict`, `safe_to_agent_run`, `exit_reason`, `why`, `blockers`, `coverage`, `findings`, `policy_actions`) | ✅ Done | `struct AgentGateJson`; test `agent_gate_json_exposes_stable_verdict_shape` (black-box) |
| 3 | F | Exit code `0` iff `SAFE_TO_AGENT_RUN: yes`, non-zero for `CAUTION`/`REJECT`/`INCOMPLETE` | ✅ Done | `tests/repo_intake_fixtures.rs` (all 10 malicious fixtures assert non-zero exit) |
| 4 | F | Supply-chain rule set: npm lifecycle scripts, manifest/lockfile gaps, git/path/http dependency sources, `build.rs` command boundaries, shell/Dockerfile download-execute, base64 decode-execute, GitHub Actions permission/trigger risk, secrets-coupled shell, unpinned Actions, Docker root/remote-repo patterns, suspicious binary/archive artifacts | ✅ Done | 21 fixtures under `tests/fixtures/repo-intake/`, exercised by `sift eval-corpus` (`eval_cases`, 21 cases) and `tests/repo_intake_fixtures.rs` |
| 5 | F | Project-local `sift-policy.toml` (`max_candidate_files`, `[[allowlist]]`, `[[denylist]]`, `[[severity_override]]`) | ✅ Done | `load_policy_config`/`parse_policy_config` in `config.rs`; test `parses_policy_schema_and_rejects_bad_severity`; `apply_policy`/`policy_match`/`policy_override_match` in `report.rs` |
| 6 | F | Suspicious binary/archive artifact inventory | ✅ Done | `inspect_suspicious_artifact`, `is_binary_or_archive_name`; fixtures `binary-artifact-exec`, `binary-extension`, `archive-payload` |
| 7 | F | `sift eval-corpus`: ≥20-case precision table | ✅ Done | `run_eval_corpus`, 21 `eval_cases`; test `eval_corpus_reports_twenty_or_more_cases` |
| 8 | G | Recent regression fixes: Cargo.lock registry source no longer flagged as a git dependency; `workflow-write-all` no longer conflates single-scope `contents:`/`actions:`/`packages: write` with broad `write-all`; `record_truncated > 0` no longer forces `INCOMPLETE` by itself; VCS metadata dirs (`.git`, `.hg`, `.svn`, `.jj`) excluded from scan | ✅ Done | Landed in current HEAD `f9a374b`, superseding the open items in `reports/project-audit-2026-07-01.md` (written against parent commit `88c5334`). Evidence: tests `ignores_cargo_lock_crates_io_registry_source`, `flags_broad_but_not_scoped_workflow_write_permissions`; `scanner.rs::VCS_METADATA_DIRS`; `report.rs::gate_incomplete_reasons` no longer reads `record_truncated` |
| 9 | ⛑ | **Dogfood finding, fixed this session:** `sift . --agent-gate` on sift's own repository returned `CAUTION` due to two real bugs, both now fixed — see [Self-audit dogfood check](#self-audit-dogfood-check) | ✅ Done | (a) `looks_like_eval_invocation` added to `report.rs`/`extract.rs`, requiring a shell-substitution token after a standalone `eval` word so English prose like "eval corpus" no longer trips `dynamic-shell-eval`; tests `flags_real_dynamic_shell_eval_invocation`, `ignores_eval_used_as_an_english_word`, `markdown_prose_mentioning_eval_corpus_is_not_a_command`. (b) `[[allowlist]]` policy matching extended from `RiskFinding`s to `coverage.suspicious_artifacts` via `apply_policy_to_artifacts`/`policy_match_artifact`, plus a new root `sift-policy.toml` allowlisting `.githooks/pre-commit` and the `tests/fixtures/repo-intake/` synthetic artifacts; tests `policy_allowlist_suppresses_matching_suspicious_artifact`, `policy_allowlisting_every_artifact_reaches_accept`, `policy_allowlist_matches_one_tag_within_a_combined_artifact_reason`. Re-run after both fixes: 0 blockers, but verdict is still `CAUTION` — this is now understood to be correct, not a bug (see dogfood section) |

### P4c — Operational modes

| # | Type | Item | Status | Evidence |
|---|---|------|--------|----------|
| 1 | F | `--benchmark` local telemetry (no model calls; optional USD cost estimate) | ✅ Done | `tests/benchmark_mode.rs` (3/3 passing) |
| 2 | F | `sift github owner/repo` safe intake — never builds, installs, runs hooks, or touches submodules; inspects file/byte limits, `.gitmodules`, Git LFS before scanning | ✅ Done | `run_github_intake`, `parse_github_repo`, `inspect_checkout_dir`; tests `github_repo_parser_accepts_owner_repo_and_https`, `checkout_inspection_reports_lfs_and_limits`, `github_intake_rejects_non_github_url_without_network` (black-box). Both `git` fetch and the recursive local `sift` invocation run under `run_command_with_timeout` (120s / 600s hard deadlines with kill-on-timeout) |
| 3 | F | `sift doctor` — config/key/endpoint diagnostics | 🟡 Partial | Implemented (`run_doctor`, `check_config_permissions`, `check_file_config`, `check_endpoint_key_pair`, …) but **has zero automated test coverage** — no unit test in `config.rs::tests` exercises `run_doctor`/`Doctor`, and no integration test in `tests/` spawns `sift doctor`. The internal gate's "each file has `#[cfg(test)]`" check (BT) passes for `config.rs` only because *other* functions in the same file are tested — it cannot see this gap |
| 4 | F | `--save`/`--save-to` persisted reports (`reports/sift-audit-result-YYYYMMDD-NNN.md`) | ✅ Done | `save_audit_result`, `next_audit_result_path`, `utc_yyyymmdd`, `civil_from_days` in `main.rs` |
| 5 | F | `--report-language {en,zh}` bilingual Markdown reports | ✅ Done | `ReportLanguage`; test `localized_headings_render_for_zh` |
| 6 | F | `--debug` extra stderr diagnostics | ✅ Done | `main.rs` debug `eprintln!` blocks |
| 7 | B | Small-model Map (`map_small_pool`) is retained as inactive diagnostic scaffolding, not called by the default full-audit path | 🟡 Partial (by design) | Code + 4 tests exist in `model.rs` (`small_pool_maps_successful_observations`, etc.), but `main.rs` prints `"small-model Map inactive: reduce converges from deterministic findings"` and never calls it. This matches AGENT.md's framing exactly — it is correctly labeled scaffolding, not a defect — but it is still an **open roadmap decision**: reintroduce behind a behavior-level gate, or retire it |

**Phase verdict: 🟡 Mostly done — matches the project's own "P4 in progress" self-report.** The two genuinely open engineering items are #10 in P4a (no CI-automated full-audit smoke) and #3 in P4c (`doctor` untested); the small-model Map question (#7 in P4c) is an intentional open decision, not a bug.

---

## P5 — Internal Quality Gate

ROADMAP status: heading now carries the ✓ (updated this session); the feature and its gate are fully built and green.

| # | Type | Item | Status | Evidence |
|---|---|------|--------|----------|
| 1 | F | `audit.rs` self-audit module scoring dimensions CQ/SEC/RB/DF/BT/CC/UX | ✅ Done | `src/audit.rs::run_checks` (13 checks) |
| 2 | F | Writes a maintainer-only report to `reports/internal-gate.md` (gitignored) | ✅ Done | `write_internal_gate`; `.gitignore` contains `/reports/` |
| 3 | F | Hidden from the public CLI (triggered by `SIFT_INTERNAL_GATE=1`, not a documented flag) | ✅ Done | `internal_gate_target()` in `main.rs`; test `self_audit_flag_is_not_public_cli_argument` confirms no `--self-audit` flag exists |
| 4 | F | Wired into `make internal-gate` / `make ci` | ✅ Done | `Makefile`; verified this session (`make ci` exit 0) |
| 5 | G | No FAIL/WARN for hard rules, including no broad `dead_code` allow, no raw CJK source literals, clean report-stream boundary, visible seed truncation | ✅ Done | This session's fresh run: **13/13 PASS, 0 WARN, 0 FAIL** (`reports/internal-gate.md`) |
| 6 | ⚑ | Test-coverage check (`BT`) is file-granularity only | 🟡 Known limitation | `test_coverage_status` only checks that a file contains `#[cfg(test)]` *somewhere* — it cannot detect that a specific function (e.g., `run_doctor`) is untested inside an otherwise-tested file. See P4c #3 |

**Phase verdict: ✅ Done.** `ROADMAP.md`/`ROADMAP.zh.md` P5 headings were updated to `— done ✓` this session to match this evidence. Remaining suggestion: tighten the `BT` check toward function-level coverage.

---

## P6 — Release hardening

ROADMAP status: no checkmark in the heading; substantial evidence exists.

| # | Type | Item | Status | Evidence |
|---|---|------|--------|----------|
| 1 | F | Size-tuned release profile (`opt-level=z`, `lto`, `codegen-units=1`, `strip`, `panic=abort`) | ✅ Done | `Cargo.toml::[profile.release]` |
| 2 | F | Makefile install/uninstall path (`~/.local/bin` default, `PREFIX`/`BINDIR` overrides) | ✅ Done | `Makefile` `install`/`uninstall` targets |
| 3 | F | Git hooks install/uninstall; pre-commit runs `make local-ci` | ✅ Done | `Makefile` `githooks-install`/`githooks-uninstall`; `.githooks/pre-commit` |
| 4 | F | CI: fmt/test/clippy/internal-gate on an `ubuntu-latest` + `macos-latest` matrix | ✅ Done | `.github/workflows/ci.yml` |
| 5 | F | Release workflow: SemVer tag guard, macOS amd64/arm64 build, `tar.xz` + `sha256`, environment-gated draft→published GitHub release | ✅ Done | `.github/workflows/release.yml`; tags `v0.1.0`, `v0.2.0` exist |
| 6 | F | Homebrew tap auto-publish (`jamiesun/homebrew-tap` formula render + push) | ✅ Done | `release.yml::homebrew` job; depends on the `HOMEBREW_TAP_TOKEN` repo secret being configured, which is outside this repo's own verifiable scope |
| 7 | F | More grammars | ⏳ Pending (open-ended) | 23 tree-sitter grammars + 4 structural extractors already shipped (see P1); ROADMAP intentionally leaves this unbounded, so it can never be marked fully "done" |
| 8 | F | Stable JSON output contracts (`schema_version`) across `--benchmark`, `--agent-gate --format json`, `eval-corpus` | ✅ Done | `schema_version: 1` asserted in `benchmark_mode_outputs_stable_json_without_model_keys`, `agent_gate_json_exposes_stable_verdict_shape` |
| 9 | G | Single-file dist | ✅ Done | `release.yml` packages one `sift` binary (+ docs/README/config template) per `tar.xz` |
| 10 | G | Internal gates pass | ✅ Done | See P5 |
| 11 | G | Docs ↔ code consistent | 🟡 Partial (manual only) | No automated check diffs documentation (supported-language lists, CLI flags, version strings) against source of truth; verified by manual cross-reading this session, but **nothing in `make ci` would catch future drift** |
| 12 | G | `brew install jamiesun/tap/sift` backed by release checksums | ✅ Done (unverified externally) | `sha256`/formula-render logic present in `release.yml`; not independently re-checked against the live `jamiesun/homebrew-tap` repository in this session |

**Phase verdict: 🟡 Mostly done.** Two open threads: docs↔code consistency has no automated guard, and "more grammars" is an intentionally unbounded target rather than a gate to close.

---

## Cross-cutting: Engineering Contract (ROADMAP.md)

| # | Rule | Status | Evidence |
|---|------|--------|----------|
| 1 | A phase marked done has behavior-level proof, not just type-level plumbing | ✅ Held for P0–P3; 🟡 two exceptions noted above (P0 #7, P4c #3) |
| 2 | Full-audit stdout is the final report; `--scan-only` is JSONL; diagnostics stay off stdout | ✅ Done | See P4a #7 |
| 3 | Report discloses how much input was scanned/dehydrated/sent/skipped/truncated | ✅ Done | `InputCoverage`, `AgentGateCoverage` |
| 4 | Missing user config auto-created from safe defaults; an invalid config file fails instead of reverting to defaults | ✅ Done | See P4a #8 |
| 5 | `src/` is English-only for runtime text, prompts, and comments | ✅ Done | internal-gate PASS "Program source avoids raw CJK literals" |

## Cross-cutting: Definition of Done (ROADMAP.md)

| # | Criterion | Status |
|---|-----------|--------|
| 1 | Zero-config run; `~/.sift/config.toml` auto-created; missing key exits with a hint; never hangs | ✅ Done |
| 2 | 100 MB repo stable memory; no crash on dirty input | ⬜ Not done — see P1 #7 |
| 3 | Report cites line numbers + cross-module deps + concurrency/resource risk | ✅ Done |
| 4 | Report declares input coverage and truncation state; incomplete coverage never looks like a complete verdict | ✅ Done |
| 5 | Every external call times out; failures trip to partial, never grind | ✅ Done — model HTTP calls (`model.rs`) and GitHub-intake subprocesses (`run_command_with_timeout`, 120s/600s) both verified |
| 6 | One binary audits project and `--module` without bleed | ✅ Done |
| 7 | Internal release gates have no FAIL or hard-rule WARN | ✅ Done |

---

## Non-goals guardrail

Confirms none of ROADMAP.md's hard "must never do" rules have been crossed.

| # | Non-goal | Held? | Evidence |
|---|----------|-------|----------|
| 1 | No vector DB / embeddings / RAG | ✅ Held | `Cargo.toml` dependency list has no vector-DB/embedding crate |
| 2 | No runtime plugins / dynamic skills | ✅ Held | `skills.rs::Skill` is a compile-time `enum` + `match`; no dynamic-loading dependency |
| 3 | No service / Web UI / multi-tenant | ✅ Held | No web-server crate in `Cargo.toml`; CLI-only via `clap` |
| 4 | No process panics | ✅ Held (heuristic, not formal) | internal-gate PASS on both explicit `panic!` and `unwrap()`/`expect()` literal-pattern checks. Note: `panic = "abort"` in the release profile changes unwind behavior *if* a panic ever happens — it is not itself a no-panic guarantee. The real guarantee is the source-text scan, which cannot catch e.g. indexing/overflow panics |
| 5 | No unbounded blocking | ✅ Held | Model calls: `ureq` timeout in `model.rs`. Subprocesses: `run_command_with_timeout` (git fetch 120s, recursive local `sift` invocation 600s, kill-on-timeout) |
| 6 | Module audit must not balloon to global | ✅ Held | See P4a #9 |
| 7 | No trial-run instead of audit | ✅ Held | `sift github` never builds/installs/runs hooks/submodules regardless of flags; `--no-build`/`--no-install` on `GithubCli` are explicit safety-intent markers, not toggles — the tool never builds or installs either way |
| 8 | No scaffold masquerading as product | ✅ Held | Small-model Map is explicitly labeled "inactive diagnostic scaffolding" in both code output and docs, not counted as shipped default behavior |
| 9 | No silent fallback | ✅ Held | See P4a #8; invalid config always fails loudly |

---

## Self-audit dogfood check

AGENT.md states "sift itself must pass its internal release gates." That claim covers **two different gates**, which this snapshot deliberately keeps separate:

1. **Internal quality gate** (`SIFT_INTERNAL_GATE=1`, i.e. `make internal-gate`) — sift's own *code-quality* gate. **Result: 13/13 PASS, 0 FAIL, 0 WARN.** ✅ This is the gate ROADMAP.md and AGENT.md are talking about, and it is green.
2. **Agent gate** (`sift . --agent-gate`) — the *product feature* meant to screen arbitrary third-party repositories before an agent runs setup/build/install. There is no roadmap requirement that sift accepts its own repository under this gate, but running it is a useful dogfood check.

### First run (start of this session): two real bugs found

```text
VERDICT: CAUTION
SAFE_TO_AGENT_RUN: no
coverage: candidate_files=69 dehydrated_files=62 unsupported_files=7
          record_truncated=12 seed_bytes=148402
```

- **Rule false positive:** `docs/ROADMAP.zh.md` and other prose files were flagged `dynamic-shell-eval` (scope=docs, MEDIUM) purely because the English phrase **"eval corpus"** (sift's own `eval-corpus` feature name) contains the substring `"eval "`, which `looks_like_dynamic_shell_eval` (`src/report.rs`) and `looks_like_shell_command` (`src/extract.rs`) both matched unconditionally. Not a real shell-eval risk.
- **Unreviewed but legitimate artifacts:** `.githooks/pre-commit` (an extensionless, real, committed executable) and two committed test fixtures (`archive-payload/assets/payload.tar.gz`, `binary-extension/bin/tool.dylib`) tripped the suspicious-artifact rule. There was no project-local `sift-policy.toml` (only `sift-policy.example.toml`), and even with one, policy `[[allowlist]]` matching only applied to `RiskFinding`s, never to `coverage.suspicious_artifacts` — so these blockers had no suppression path at all.

### Fixes landed this session

1. Added `looks_like_eval_invocation` (word-boundary + shell-substitution-token check) in both `report.rs` and `extract.rs`, so `eval` only flags a real invocation — the standalone word immediately followed by a command substitution, backticks, or a `$variable` — and never English/Chinese prose mentioning "eval corpus"/"retrieval". Covered by `flags_real_dynamic_shell_eval_invocation`, `ignores_eval_used_as_an_english_word`, `markdown_prose_mentioning_eval_corpus_is_not_a_command`.
2. Extended the policy engine so `[[allowlist]]` also suppresses `suspicious_artifacts` blockers, matching `rule` against the artifact's `reason` tag (`apply_policy_to_artifacts`, `policy_match_artifact` in `report.rs`; handles comma-joined multi-reason artifacts too). Covered by `policy_allowlist_suppresses_matching_suspicious_artifact`, `policy_allowlisting_every_artifact_reaches_accept`, `policy_allowlist_matches_one_tag_within_a_combined_artifact_reason`.
3. Added a real root `sift-policy.toml` (previously only `sift-policy.example.toml` existed) allowlisting `.githooks/pre-commit` and the `tests/fixtures/repo-intake/` synthetic artifacts, each with a written reason. `sift-policy.example.toml` was extended with a documented example of the new artifact-allowlist form.

### Second run (after fixes): blockers gone, verdict still (correctly) CAUTION

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

(Note for future editors of this very section: describing these two rules' trigger shapes in a literal, directly reproducible way can make this file itself trip them. Keep any such illustrative examples suitably paraphrased.)

Both root causes are fixed and verified: the eval false positive is gone (the one remaining `dynamic-shell-eval` finding is a real shell-invocation fixture — `bash` with an inline `-c` command interpolating a secret — exactly as intended), and all three unreviewed-artifact blockers are now suppressed with a written, reviewed reason.

**The verdict nonetheless stays `CAUTION`, and this is now understood to be correct — not a defect to chase.** All 40 remaining findings are `Severity::Low`, none `Medium`/`High`, and every one traces to one of two intentional, by-design sources:

- The 21 synthetic attack-pattern fixtures under `tests/fixtures/repo-intake/` (the same corpus `sift eval-corpus` scores). They exist specifically to prove the supply-chain rule engine detects `npm-lifecycle-script`, `download-execute`, `dependency-git-source`, `workflow-write-all`, etc. If self-scanning made these disappear, the rules would be broken, not fixed.
- `panic-edge` (`.expect()`/`.unwrap()`) findings inside `tests/*.rs`. Hard Rule #1 forbids `unwrap()`/`expect()` only in `src/`; using them in tests is normal and correct, and `PathScope::classify` already caps these to Low — they still show up as findings (informational), they just cannot be silently hidden.

The agent gate's verdict rule (`render_agent_gate`) only returns `ACCEPT` when `findings` is completely empty. Forcing that for sift's own repository would require either deleting its own regression corpus or blanket-allowlisting every rule across `tests/`, both of which would remove the evidence this checklist's P4a/P4b rows cite. The honest, durable dogfood claim is therefore: **0 High findings, 0 unexplained blockers, every Low finding accounted for** — not a literal `ACCEPT`.

---

## Consolidated open items

Everything not marked ✅ Done above, in one place. Two items from the previous snapshot were resolved this session and are omitted here (agent-gate self-CAUTION root causes fixed; ROADMAP P5 heading refreshed) — see [Self-audit dogfood check](#self-audit-dogfood-check) for the former.

| Item | Phase | Status | Suggested next step |
|------|-------|--------|----------------------|
| No black-box test asserts exit code 1 for a real full-audit run with no key | P0 | 🟡 Partial | Add an integration test under `tests/` |
| No 100 MB stress fixture; macOS resident-memory metric is always `"unavailable"` | P1 | ⬜ Not done | Add a large-corpus smoke test; extend `resident_memory_metric` to macOS (`task_info`/`ps`) |
| Fake-endpoint full-audit smoke is manual-only, not CI-automated, and predates the current small-model-Map-inactive default | P4a | 🟡 Partial | Add a mock-HTTP-server integration test exercising `react::ReAct` end to end |
| The pre-existing policy-suppression logic (`apply_policy`/`policy_match`/`policy_override_match` for `RiskFinding`s) has no direct unit test exercising suppression end-to-end — only TOML parsing is tested (`parses_policy_schema_and_rejects_bad_severity`). The new artifact-allowlist path added this session is tested; the original finding-allowlist path still is not | P4b | 🟡 Partial | Add `apply_policy`/denylist/severity-override unit tests in `report.rs`, mirroring the new `policy_allowlist_*` artifact tests |
| `sift doctor` has zero automated test coverage | P4c | 🟡 Partial | Add unit tests for `Doctor`/`run_doctor` and/or a `tests/doctor.rs` black-box test |
| Small-model Map is inactive scaffolding; reintroduce-or-retire decision is still open | P4c | 🟡 Partial (by design) | Maintainer decision, then either wire behind a behavior-level gate or delete |
| "More grammars" has no fixed target | P6 | ⏳ Pending | Not a defect; track via issues per language request instead of this checklist |
| Docs ↔ code consistency has no automated guard | P6 | 🟡 Partial | Consider an `audit.rs` check that greps `README.md`'s supported-language list against `extract.rs::Lang` variants |

---

## Refreshing this snapshot

```sh
cargo build
make ci                                   # fmt-check + test + clippy -D warnings + internal-gate
cat reports/internal-gate.md              # P5 gate detail (gitignored, local only)
cargo run --quiet -- . --agent-gate --format json   # live self-scan (dogfood check above)
sift eval-corpus                          # repo-intake precision table
```

This file reflects one commit in time. Re-run the commands above and update the
Snapshot table, the phase tables, and the Consolidated open items whenever a
phase's evidence changes — do not hand-edit a status mark without re-checking
its evidence.
