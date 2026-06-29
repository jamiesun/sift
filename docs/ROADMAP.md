# sift Project Profile & Roadmap

> English | [中文](ROADMAP.zh.md)
>
> North star + guardrails + phased build boundaries. Defines what it should become / what it must never do / what each phase ships / when it can self-audit.
> Name: **sift** (CLI is `sift`). Language: Rust.

## Overview

A **cost-controlled** open-source project auditor. Before adopting a library, get a file/line-level risk ledger without trial-running it or force-feeding tens of thousands of lines into a frontier model.

Core: **tiered funnel + compute mismatch + ReACT scheduling**. Grunt work (structure extraction, coarse filtering) goes to zero-cost static parsing and cheap small models; heavy logic convergence goes to a frontier model; a ReACT state machine orchestrates both. Ships as a single binary, zero-config, auditing a **whole project** or a **single module**. **sift itself must pass a sift audit.**

- Architecture

```text
  CLI key file / ENV / config.toml ──(fallback resolve, exit if no key)
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
  Self-audit  sift audits sift + 10-dim scored gate               [P5/P6]
```

## Project Profile (target state)

- **Zero-friction cold start.** `sift ./repo --scan-only` just runs; no interactive prompts; exits with an injection hint if the key is missing.
- **Cost-controlled & budgetable.** Tokens mostly spent on small models; the large model only sees the dehydrated skeleton.
- **Big/small co-scheduling.** A ReACT state machine chains coarse filter (small) and convergence (large); skills are compile-time local functions.
- **Multi-model + concurrency.** Multiple endpoints configurable; small-model pool runs Map concurrently; large model converges once.
- **Never grind blindly.** Every external call has a hard timeout; repeated failures trip the breaker; on trip, back off / degrade or emit a partial report — never hang.
- **Memory decoupled from scale.** Stream and drop; resident memory stays low.
- **Self-auditable.** `sift .` must pass its own audit; modular, TDD-guarded, clear boundaries.
- **Priority on conflict:** robust > usable report > cheap > fast > small.

## Non-goals (hard rules)

- **No vector DB / embeddings / RAG.** For one-shot low-frequency audits, index upkeep costs more than prompt assembly; plain-text pipeline, read once and discard.
- **No runtime plugins / dynamic skills.** Skills = compile-time enum + match local fns; extend by editing and recompiling.
- **No service / Web UI / multi-tenant.** One-shot CLI only.
- **No process panics.** Dirty data dropped & logged; hallucinations/bad JSON tripped; Result/Option throughout, no unwrap/expect.
- **No unbounded blocking.** Any subprocess/network/model call must have a deadline.
- **Module audit must not balloon to global.** Cross-boundary refs marked and handed to the large model; no chasing.
- **No trial-run instead of audit.** Value is the pre-adoption verdict.

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
src/report.rs     Markdown risk-list renderer                  [P4 scaffold]
src/audit.rs      self-audit dimension scoring                 [P5]
```

## Multi-model & concurrency (config schema)

```toml
concurrency = 8          # small-model concurrency cap
[[model]]
role = "small"           # small=filter pool / large=convergence
endpoint = "..."; key_env = "SIFT_SMALL_KEY"
timeout_ms = 8000; max_retries = 1
[[model]]
role = "large"
endpoint = "..."; key_env = "SIFT_API_KEY"
timeout_ms = 60000; max_retries = 1
```
Resolve order: CLI key file > ENV > toml > default; no large key ⇒ exit. Missing small model degrades to AST-only fallback.

## Timeout, breaker & recovery (never grind)

- **Per-call deadline:** time out and drop; no unbounded wait.
- **Breaker counter:** consecutive failures / bad JSON / unknown skill ≥ N ⇒ break, stop I/O.
- **Backoff recovery:** transient errors retry with exponential backoff to budget; non-transient degrade (small→AST, large→partial).
- **Budget cap:** global token/time ceiling; on hit, force-converge a `[TRUNCATED]` report.

## Phased Roadmap

> Each phase: feature list / boundaries / self-audit gate. All-green gate ⇒ next phase; next steps set by audit result.

### P0 Scaffold — done ✓
Features: clap fallback resolve, bounded scanner, exit on missing key, placeholders. Bounds: no net/parse/tree. Gate: `cargo build` green, 0 unwrap, `--scan-only` scans, missing key exit1.

### P1 Tier-0 AST dehydrate — done ✓
Features: tree-sitter Rust+Python, extract sig/import/calls → flat AstSummary JSON; cross-boundary `[EXTERNAL_BLACKBOX]`; drop AST. Bounds: drop bodies/comments, drop bad nodes silently. Gate: 100MB repo memory stable & no crash; extract.rs tests cover typical+broken.

### P2 Model layer (multi-model + breaker) — done ✓
Features: ModelClient trait, registry, role routing; per-call timeout, breaker, backoff. Bounds: no cache/persist; keys env/file only, never logged. Gate: timeout/bad-response simulated, breaker trips; no plaintext keys.

### P3 ReACT scheduler — done ✓
Features: enum state machine, initial tool protocol prompt, large model emits `<TOOL_CALL>`, match-routes local skills via `$SEED`; retry≤N then partial. Bounds: compile-time skills, no dynamic load. Gate: bad JSON/unknown skill/N errors all trip; react.rs tested. Small-pool concurrency wired by P4.

### P4 Map+Reduce+report
Features: deterministic AST coarse ledger, Markdown renderer, `[[model]]` config parsing, and small-pool Map waves are scaffolded; next is seeded-risk report gates and stronger large convergence. Bounds: module mode slices root only. Gate: hits seeded risks; module/project don't bleed.

### P5 Self-audit
Features: audit.rs scores trimmed 10 dims; `sift . --self-audit` writes report to `reports/` (gitignored). Gate: self-audit no FAIL.

### P6 Release hardening
Features: ReleaseSmall single binary, more grammars, stable JSON. Gate: single-file dist, self-audit PASS, docs↔code consistent.

## Definition of done

- Zero-config run; missing key exits with hint; never hangs.
- 100MB repo stable memory; no crash on dirty input.
- Report cites line numbers + cross-module deps + concurrency/resource risk.
- Every external call times out; failures trip to partial, never grind.
- One binary audits project and `--module` without bleed.
- `sift . --self-audit` self-audit no FAIL.

> Suggestions (not rules): rayon, exact timeout/size/latency numbers per benchmark. Hard rules: single binary, fallback resolve, bounded channel, hard-timeout breaker, no unwrap, TDD, bilingual docs (EN default, ZH twin), passing self-audit.
