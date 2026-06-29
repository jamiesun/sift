# sift

> English | [中文](docs/README.zh.md)

Cost-controlled open-source project auditor: **tiered funnel + compute mismatch + ReACT scheduling**. Before adopting a dependency, get a file/line-level risk ledger without force-feeding tens of thousands of lines into a frontier model.

- Grunt work (structure extraction / coarse filtering) → tree-sitter + cheap small models
- Logic convergence → frontier large model, orchestrated by a ReACT state machine
- Single binary, zero-config; audits a whole project or a single module; sift must pass a sift audit

See [docs/ROADMAP.md](docs/ROADMAP.md) for full design.

## Usage

```sh
sift ./repo --scan-only        # scan layer only (no key needed)
sift ./repo --module src        # audit a submodule
SIFT_API_KEY=<KEY> sift ./repo  # full pipeline
sift ./repo --api-key-file ~/.config/sift/key
sift ./repo --self-audit        # local P5 gate, no model key needed
```

## Status

P0 scaffold + P1 AST dehydrate + P2 model layer + P3 ReACT scheduler (tool protocol, compile-time skills, retry→partial) done. P4 is in progress: local AST risk ledger, Markdown renderer, `[[model]]` config parsing, and small-model Map waves are wired. A minimal P5 local self-audit now writes `reports/self-audit.md`; seeded report gates and stronger scoring come next.

## Docs

- [Roadmap](docs/ROADMAP.md) · [路线图](docs/ROADMAP.zh.md)
- [Contributor handbook (AGENT.md)](AGENT.md) · [中文](docs/AGENT.zh.md)
