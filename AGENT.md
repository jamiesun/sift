# AGENT.md — sift Contributor Handbook

> English | [中文](docs/AGENT.zh.md)
>
> The implementation handbook for humans and agents working on sift. Source of truth for hard rules, layout, and habits. Profile/boundaries live in [docs/ROADMAP.md](docs/ROADMAP.md).

## What sift is

A cost-controlled, single-binary open-source auditor: tree-sitter dehydration → small-model coarse filter (Map) → large-model convergence (Reduce), orchestrated by a ReACT state machine. Audits a whole project or one module. **sift must pass `sift .`.**

## Hard Rules

1. **No `unwrap()` / `expect()` in `src/`.** Dirty data takes a `Result`/`Option` branch and is dropped+logged; the main process never panics.
2. **Every external call (subprocess/network/model) has a hard timeout.** Unbounded blocking is a bug. Repeated failure trips a breaker; on trip, back off / degrade / emit partial — never grind.
3. **Single binary, low deps.** No vector DB, no embeddings/RAG, no DB, no cache. Plain-text pipeline, read once and discard.
4. **Compile-time skills only.** Skills = `enum` + `match` local functions; no dynamic loading or runtime plugins.
5. **Streaming, memory decoupled from scale.** Bounded channel, drop the AST after dehydrating; resident memory stays low.
6. **Fallback key resolution.** CLI key file > ENV > config.toml > default; missing large key exits immediately with a hint, never hangs or prompts.
7. **Secrets via env/file only.** Never compiled in, committed, printed, or logged.
8. **Module audit must not balloon to global.** Cross-boundary refs marked `[EXTERNAL_BLACKBOX]`; do not chase.
9. **TDD.** Each `src/*.rs` carries unit tests; build tests alongside new subsystems.
10. **Bilingual docs, English default.** Every doc has a ZH twin (`docs/*.zh.md`); EN is canonical, scope/commands/rules must match across languages.
11. **No toy gates or fake capability claims.** Scaffold code must be named as scaffold, isolated behind explicit modes, and must not be counted as a completed phase until behavior-level gates prove it.
12. **Stable output contracts.** `--scan-only` may write JSONL to stdout; full audit stdout is reserved for the final report. Progress, diagnostics, and model telemetry go to stderr or reports, never mixed into the report stream.
13. **No silent degradation.** Truncation, skipped files, model fallback, partial reports, invalid config, and parse failures must be visible in output, exit status, or self-audit evidence. Invalid config files fail; they do not quietly revert to defaults.
14. **Program source is English-only.** Runtime strings, prompts, and source comments in `src/` are English. Bilingual user docs stay in `docs/*.zh.md`.

> Any Hard Rule violation is an automatic FAIL in self-audit.

## Code Map

| Path | Role | Phase |
|------|------|-------|
| `src/main.rs` | wiring: parse→Config→schedule→report→exit | P0 ✓ |
| `src/config.rs` | fallback resolve, multi-model config | P0 ✓→P2 |
| `src/scanner.rs` | Walk + bounded channel | P0 ✓ |
| `src/extract.rs` | tree-sitter dehydrate → AstSummary | P1 ✓ |
| `src/model.rs` | model registry/client/timeout/breaker | P2 ✓ |
| `src/react.rs` | ReACT state machine + skill match | P3 ✓ |
| `src/skills.rs` | local skill fns (map/reduce) | P3 ✓→P4 |
| `src/report.rs` | Markdown risk-list | P4 |
| `src/audit.rs` | self-audit scoring | P5 |

## Workflow

```sh
cargo build                    # must be green
cargo test                     # tests must pass
cargo fmt && cargo clippy      # clean before commit
rg 'unwrap\(|expect\(|panic!' src  # must be 0
rg '[\p{Han}]' src             # must be 0
sift . --self-audit            # local self-audit (P5+) must be no FAIL
```

- One concern per commit; include the `Co-authored-by: Copilot` trailer.
- Before adding a feature, check it doesn't cross a ROADMAP non-goal; if it does, change the rule first.
- A phase isn't done until its self-audit gate and at least one behavior-level smoke for the user-facing path are green.
- If a phase uses scaffolding, the docs and code must say exactly what remains incomplete.

## Habits

- `Result`/`Option` everywhere; bound every wait; drop scratch eagerly.
- Keep modules at their listed responsibility; don't cross layers.
- Reach for ecosystem crates over hand-rolling, but no heavyweight deps.
- Reports go to `reports/` (gitignored); never dirty tracked files in an audit.
- Prefer failing loudly over producing a clean-looking but incomplete audit.
