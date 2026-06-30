# sift Project Profile & Roadmap

> English | [中文](ROADMAP.zh.md)
>
> North star + guardrails + phased build boundaries. Defines what it should become / what it must never do / what each phase ships / when internal gates apply.
> Name: **sift** (CLI is `sift`). Language: Rust.

## Overview

A **cost-controlled** open-source project auditor. Before adopting a library, get a file/line-level risk ledger without trial-running it or force-feeding tens of thousands of lines into a frontier model.

Core: **tiered funnel + compute mismatch + ReACT scheduling**. Grunt work (structure extraction, coarse filtering) goes to zero-cost static parsing and cheap small models; heavy logic convergence goes to a frontier model; a ReACT state machine orchestrates both. Ships as a single binary, zero-config, auditing a **whole project** or a **single module**. **sift itself must pass its internal release gates.**

- Architecture

```text
  CLI key file / ENV / ~/.sift/config.toml ──(fallback resolve, exit if no key)
        ▼
  Scan      ignore::Walk → bounded channel (consume & drop)        [P0 ✓]
        ▼
  Tier-0    tree-sitter dehydrate (sig/import/calls) → JSON → drop AST  [P1 ✓]
        │   cross-boundary refs marked [EXTERNAL_BLACKBOX]
        ▼
  Models    multi-model registry · per-call hard timeout · breaker+backoff  [P2 ✓]
        ▼  ┌─ small-model pool (concurrent Map filter) ─┐
  ReACT scheduler (tool protocol, skills=local fns, retry≤N)        [P3 ✓]
        │  └─ large model (Reduce convergence) ─────────┘
        ▼
  Report    stdout Markdown risk list (line/call-chain)            [P4 started]
        ▼
  Internal gate  scored source checks + release evidence          [P5/P6]
```

## Project Profile (target state)

- **Zero-friction cold start.** `sift ./repo --scan-only` just runs; missing `~/.sift/config.toml` is created with non-secret defaults; no interactive prompts; exits with an injection hint if the key is missing.
- **Cost-controlled & budgetable.** Tokens mostly spent on small models; the large model only sees the dehydrated skeleton.
- **Big/small co-scheduling.** A ReACT state machine chains coarse filter (small) and convergence (large); skills are compile-time local functions.
- **Multi-model + concurrency.** Multiple endpoints configurable; small-model pool runs Map concurrently; large model converges once.
- **Never grind blindly.** Every external call has a hard timeout; repeated failures trip the breaker; on trip, back off / degrade or emit a partial report — never hang.
- **Engineering-grade by default.** A clean-looking but incomplete audit is a defect. Any skipped input, truncation, fallback, partial model result, or invalid config must be visible and testable.
- **Stable machine contracts.** Scan JSONL, final Markdown, diagnostics, and generated reports have separate channels. Downstream scripts must be able to consume stdout without guessing whether it contains mixed formats.
- **Memory decoupled from scale.** Stream and drop; resident memory stays low.
- **Internally gated.** The project must pass its own maintainer-only release gates; modular, TDD-guarded, clear boundaries.
- **Priority on conflict:** robust > usable report > cheap > fast > small.

## Non-goals (hard rules)

- **No vector DB / embeddings / RAG.** For one-shot low-frequency audits, index upkeep costs more than prompt assembly; plain-text pipeline, read once and discard.
- **No runtime plugins / dynamic skills.** Skills = compile-time enum + match local fns; extend by editing and recompiling.
- **No service / Web UI / multi-tenant.** One-shot CLI only.
- **No process panics.** Dirty data dropped & logged; hallucinations/bad JSON tripped; Result/Option throughout, no unwrap/expect.
- **No unbounded blocking.** Any subprocess/network/model call must have a deadline.
- **Module audit must not balloon to global.** Cross-boundary refs marked and handed to the large model; no chasing.
- **No trial-run instead of audit.** Value is the pre-adoption verdict.
- **No scaffold masquerading as product.** Placeholders are allowed only inside explicitly unfinished phases; they must not produce reports that look production-complete.
- **No silent fallback.** Invalid config, truncated seed, skipped files, missing model roles, and degraded model paths must fail loudly or be shown in the report.

## Code Map

> Every `src/*.rs` carries unit tests; new subsystem ⇒ tests built alongside (TDD). Module boundaries are responsibility boundaries.

```text
src/main.rs       entry wiring: parse→Config→schedule→report→exit code
src/config.rs     fallback resolve, multi-model config         [P0✓→P2]
src/scanner.rs    Walk + bounded channel                       [P0✓]
src/extract.rs    tree-sitter dehydrate → AstSummary           [P1✓]
src/model.rs      multi-model registry/client trait/timeout    [P2✓]
src/react.rs      ReACT state machine + skill enum/match       [P3 ✓]
src/skills.rs     local skill fns (map filter / reduce)        [P3 ✓→P4]
src/report.rs     Markdown risk-list renderer                  [P4]
src/audit.rs      internal gate dimension scoring              [P5]
```

## Multi-model & concurrency (config schema)

```toml
concurrency = 8          # small-model concurrency cap
[[model]]
role = "small"           # small=filter pool / large=convergence
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
Resolve order: CLI key file > ENV > toml > default; no large key ⇒ exit. Missing small model degrades to AST-only fallback.
The default user config path is `~/.sift/config.toml`; it is created on first run from `config.example.toml`-equivalent defaults and must not contain raw secrets.

## Timeout, breaker & recovery (never grind)

- **Per-call deadline:** time out and drop; no unbounded wait.
- **Breaker counter:** consecutive failures / bad JSON / unknown skill ≥ N ⇒ break, stop I/O.
- **Backoff recovery:** transient errors retry with exponential backoff to budget; non-transient degrade (small→AST, large→partial).
- **Budget cap:** global token/time ceiling; on hit, force-converge a `[TRUNCATED]` report.

## Engineering Contract

- A phase marked done must have behavior-level proof, not only type-level plumbing or happy-path unit tests.
- Full audit stdout is the final report stream. `--scan-only` is the JSONL stream. Diagnostics stay off stdout.
- Report coverage must disclose how much input was scanned, dehydrated, sent to models, skipped, or truncated.
- Config files are part of the trust boundary. Missing user config is auto-created from safe defaults; if a config file exists but is invalid, the process fails instead of reverting to defaults.
- Program source under `src/` is English-only for runtime text, prompts, and comments; bilingual documentation stays in docs.

## Phased Roadmap

> Each phase: feature list / boundaries / internal gate. All-green gate ⇒ next phase; next steps set by gate evidence.

### P0 Scaffold — done ✓
Features: clap fallback resolve, bounded scanner, exit on missing key, minimal wiring. Bounds: no net/parse/tree. Gate: `cargo build` green, 0 unwrap, `--scan-only` scans, missing key exit1.

### P1 Tier-0 AST dehydrate — done ✓
Features: tree-sitter Rust/Python/Go/JavaScript/TypeScript/HTML/CSS/Zig/Bash/Dart/Kotlin/Java/C/C++/C#/PHP/Swift/Ruby/SQL/Dockerfile/YAML/HCL/Vue/Svelte, extract sig/import/calls → flat AstSummary JSON; cross-boundary `[EXTERNAL_BLACKBOX]`; drop AST. Bounds: omit bodies/comments; tolerate malformed syntax without panicking and account for incomplete coverage in downstream reporting. Gate: 100MB repo memory stable & no crash; extract.rs tests cover typical+broken.

### P2 Model layer (multi-model + breaker) — done ✓
Features: ModelClient trait, registry, role routing; per-call timeout, breaker, backoff. Bounds: no cache/persist; keys env/file only, never logged. Gate: timeout/bad-response simulated, breaker trips; no plaintext keys.

### P3 ReACT scheduler — done ✓
Features: enum state machine, initial tool protocol prompt, large model emits `<TOOL_CALL>`, match-routes local skills via `$SEED`; retry≤N then partial. Bounds: compile-time skills, no dynamic load. Gate: bad JSON/unknown skill/N errors all trip; react.rs tested. Small-pool concurrency wired by P4.

### P4 Map+Reduce+report
Features: deterministic AST coarse ledger, Markdown renderer, real `[[model]]` TOML parsing, small-pool Map waves, explicit input coverage, and clean stdout boundaries. Bounds: module mode slices root only; truncation and degraded model paths must be visible. Gate: hits seeded risks; module/project don't bleed; full-audit stdout contains only the report; invalid config fails; fake-endpoint full audit smoke proves the user-facing path.

### P5 Internal Quality Gate
Features: audit.rs scores trimmed dimensions and writes maintainer-only reports to `reports/` (gitignored). Gate: no FAIL/WARN for hard rules, including no broad dead-code allows, no Chinese source strings/comments, clean report stream boundary, and visible seed truncation.

### P6 Release hardening
Features: ReleaseSafe single binary, Makefile install path, macOS Homebrew tap publishing, more grammars, stable JSON. Gate: single-file dist, internal gates pass, docs↔code consistent, `brew install jamiesun/tap/sift` backed by release checksums.

## Definition of done

- Zero-config run; `~/.sift/config.toml` auto-created; missing key exits with hint; never hangs.
- 100MB repo stable memory; no crash on dirty input.
- Report cites line numbers + cross-module deps + concurrency/resource risk.
- Report declares input coverage and truncation state; incomplete coverage never looks like a complete verdict.
- Every external call times out; failures trip to partial, never grind.
- One binary audits project and `--module` without bleed.
- Internal release gates have no FAIL or hard-rule WARN.

> Suggestions (not rules): rayon, exact timeout/size/latency numbers per benchmark. Hard rules: single binary, fallback resolve, bounded channel, hard-timeout breaker, no unwrap, TDD, bilingual docs (EN default, ZH twin), passing internal gates.
