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
sift ./repo --agent-gate       # deterministic pre-run gate (no key needed)
sift ./repo --module src        # audit a submodule
SIFT_API_KEY=<KEY> sift ./repo  # full pipeline
sift ./repo --api-key-file ~/.sift/key
sift ./repo --report-language zh # request a Simplified Chinese Markdown report
sift ./repo --debug              # print extra diagnostics to stderr
sift doctor                    # check config, key_env, and endpoint/key mismatches
sift ./repo --self-audit        # local P5 gate, no model key needed
```

`--agent-gate` is a local, deterministic repo-intake gate for agents and wrapper
scripts. It writes only this stable contract to stdout:

```text
VERDICT: ACCEPT | CAUTION | REJECT | INCOMPLETE
WHY:
- <top evidence>
BLOCKERS:
- <file:line evidence or coverage blocker>
SAFE_TO_AGENT_RUN: yes | no
```

The command exits `0` only when `SAFE_TO_AGENT_RUN: yes`; `CAUTION`,
`REJECT`, and `INCOMPLETE` exit non-zero so callers can stop before setup,
install, build, or run steps.

On first run, sift creates `~/.sift/config.toml` from the built-in default
template. The default file contains only non-secret settings; put model keys in
environment variables or pass `--api-key-file`.

Full audits keep stdout reserved for the final Markdown report. Progress,
status, and debug diagnostics are printed to stderr so long runs do not look
stalled and downstream tools can still pipe stdout safely.

## Supported Languages

The scan layer currently dehydrates Rust, Python, Go, JavaScript, TypeScript/TSX,
HTML, CSS, Zig, Bash-compatible shell files (`.sh`, `.bash`, `.zsh`), Dart,
Kotlin, Java, C/C++, C#, PHP, Swift, Ruby, SQL, Dockerfile/Containerfile, YAML,
HCL/Terraform, Vue, and Svelte.

## Install

Build from source:

```sh
make ci
make install
```

Install local git hooks:

```sh
make githooks-install
```

The pre-commit hook runs `make local-ci` before each commit. To bypass it for an
intentional emergency commit, run `SIFT_SKIP_LOCAL_CI=1 git commit ...`.

macOS releases are published through the existing tap:

```sh
brew install jamiesun/tap/sift
```

## Status

P0 scaffold + P1 AST dehydrate + P2 model layer + P3 ReACT scheduler (tool protocol, compile-time skills, retry→partial) done. P4 is in progress: local AST risk ledger, Markdown renderer, `[[model]]` config parsing, and small-model Map waves are wired. A minimal P5 local self-audit now writes `reports/self-audit.md`; seeded report gates and stronger scoring come next.

## Docs

- [Roadmap](docs/ROADMAP.md) · [路线图](docs/ROADMAP.zh.md)
- [Contributor handbook (AGENT.md)](AGENT.md) · [中文](docs/AGENT.zh.md)
