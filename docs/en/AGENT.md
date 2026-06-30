# AGENT.md - sift Contributor Handbook

> English | [中文](../AGENT.zh.md)
>
> The implementation handbook for humans and agents working on sift. Source of truth for hard rules, layout, and habits. Profile/boundaries live in [Roadmap](../ROADMAP.md).

## What sift is

A cost-controlled, single-binary open-source auditor: tree-sitter dehydration -> small-model coarse filter (Map) -> large-model convergence (Reduce), orchestrated by a ReACT state machine. Audits a whole project or one module. **sift must pass its internal release gates.**

## Hard Rules

1. **No `unwrap()` / `expect()` in `src/`.** Dirty data takes a `Result`/`Option` branch and is dropped+logged; the main process never panics.
2. **Every external call has a hard timeout.** Unbounded blocking is a bug. Repeated failure trips a breaker; on trip, back off, degrade, or emit partial output.
3. **Single binary, low deps.** No vector DB, embeddings/RAG, DB, or cache.
4. **Compile-time skills only.** Skills are an `enum` plus `match` to local functions.
5. **Streaming, memory decoupled from scale.** Bounded channel, drop the AST after dehydrating.
6. **Fallback key resolution.** CLI key file > ENV > project `.env` > `~/.sift/config.toml` > default.
7. **Secrets via env/file only.** Never compiled in, committed, printed, or logged.
8. **Module audit must not balloon to global.** Cross-boundary refs are marked `[EXTERNAL_BLACKBOX]`; do not chase.
9. **TDD.** Each `src/*.rs` carries unit tests; build tests alongside new subsystems.
10. **Bilingual docs, English default.** Every user-facing doc has a Chinese counterpart, and commands/rules must match across languages.
11. **No toy gates or fake capability claims.** Scaffold code must be named as scaffold and isolated behind explicit modes.
12. **Stable output contracts.** `--scan-only` writes JSONL to stdout; full audit stdout is reserved for the final report.
13. **No silent degradation.** Truncation, skipped files, model fallback, partial reports, invalid config, and parse failures must be visible.
14. **Program source is English-only.** Runtime strings, prompts, and comments in `src/` are English.

## Code Map

| Path | Role | Phase |
|------|------|-------|
| `src/main.rs` | wiring: parse -> Config -> schedule -> report -> exit | P0 |
| `src/config.rs` | fallback resolve, multi-model config | P0 -> P2 |
| `src/scanner.rs` | Walk + bounded channel | P0 |
| `src/extract.rs` | tree-sitter dehydrate -> AstSummary | P1 |
| `src/model.rs` | model registry/client/timeout/breaker | P2 |
| `src/react.rs` | ReACT state machine + skill match | P3 |
| `src/skills.rs` | local skill functions | P3 -> P4 |
| `src/report.rs` | Markdown risk list | P4 |
| `src/audit.rs` | internal gate scoring | P5 |

## Workflow

```sh
cargo build
cargo test
cargo fmt && cargo clippy
make ci
rg 'unwrap\(|expect\(|panic!' src
rg '[\p{Han}]' src
```

- One concern per commit.
- Before adding a feature, check it does not cross a roadmap non-goal.
- A phase is not done until its internal gate and at least one behavior-level smoke are green.
- Reports go to `reports/`, which is gitignored.
