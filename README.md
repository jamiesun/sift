# sift

> English | [中文](docs/README.zh.md)

Cost-controlled open-source project auditor: **tiered funnel + compute mismatch + ReACT scheduling**. Before adopting a dependency, get a file/line-level risk ledger without force-feeding tens of thousands of lines into a frontier model.

- Grunt work (structure extraction / coarse filtering) → tree-sitter + cheap small models
- Logic convergence → frontier large model, orchestrated by a ReACT state machine
- Single binary, zero-config; audits a whole project or a single module; sift must pass its internal release gates

See [docs/ROADMAP.md](docs/ROADMAP.md) for full design.

## Usage

```sh
sift ./repo --scan-only        # scan layer only (no key needed)
sift ./repo --agent-gate       # deterministic pre-run gate (no key needed)
sift ./repo --benchmark        # scan/model budget telemetry JSON (no key needed)
sift github owner/repo         # safe GitHub intake, defaults to --agent-gate
sift github owner/repo --ref main --scan-only
sift ./repo --module src        # audit a submodule
SIFT_API_KEY=<KEY> sift ./repo  # full pipeline
sift ./repo --api-key-file ~/.sift/key
sift ./repo --report-language zh # request a Simplified Chinese Markdown report
sift ./repo --save               # also save the report to reports/sift-audit-result-YYYYMMDD-NNN.md
sift ./repo --save-to out/audits # save the report into a custom directory (implies --save)
sift ./repo --debug              # print extra diagnostics to stderr
sift doctor                    # check config, key_env, and endpoint/key mismatches
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

The deterministic supply-chain layer currently flags npm install lifecycle
scripts, Rust `build.rs` command boundaries, shell/Dockerfile download-execute
patterns, base64 decode-to-execute flows, GitHub Actions secrets coupled to
shell blocks, and unpinned GitHub Actions.

`sift github` accepts `owner/repo` or `https://github.com/owner/repo`, fetches a
temporary checkout with `git`, resolves the commit SHA, then runs the local
scan/gate/benchmark pipeline against that checkout. It never runs repository
code, package manager commands, build scripts, hooks, install commands, or
submodules. Temporary checkouts are removed by default; use `--keep-checkout`
only when you need to inspect the fetched tree.

On first run, sift creates `~/.sift/config.toml` from the built-in default
template. The default file contains only non-secret settings; put model keys in
environment variables or pass `--api-key-file`.

Full audits keep stdout reserved for the final Markdown report. Progress,
status, and debug diagnostics are printed to stderr so long runs do not look
stalled and downstream tools can still pipe stdout safely.

`--benchmark` is a local telemetry mode for release notes and cost checks. It
does not call models; stdout is stable JSON unless `--benchmark-output <path>`
is used. The report includes candidate/dehydrated/skipped counts, scan timing,
best-available resident memory, seed bytes, planned Reduce batches, model-call
counts, approximate token counts, and optional USD cost estimates. Pricing is
explicit and never inferred:

```sh
sift ./repo --benchmark \
  --benchmark-input-1m-cost 0.25 \
  --benchmark-output-1m-cost 1.00 \
  --benchmark-estimated-output-tokens 2000
```

## Supported Languages

The scan layer currently dehydrates Rust, Python, Go, JavaScript, TypeScript/TSX,
HTML, CSS, Zig, Bash-compatible shell files (`.sh`, `.bash`, `.zsh`), Dart,
Kotlin, Java, C/C++, C#, PHP, Swift, Ruby, SQL, Dockerfile/Containerfile, YAML,
HCL/Terraform, Vue, Svelte, `package.json`, Makefile, and Markdown install
snippets.

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

## Test Fixtures

`tests/fixtures/repo-intake/` contains synthetic malicious and benign repository
trees used by the deterministic `--agent-gate` regression suite. The fixture
commands are inert examples and must never be executed as install scripts.

macOS releases are published through the existing tap:

```sh
brew install jamiesun/tap/sift
```

## Status

P0 scaffold + P1 AST dehydrate + P2 model layer + P3 ReACT scheduler (tool protocol, compile-time skills, retry→partial) done. P4 is in progress: local AST risk ledger, Markdown renderer, `[[model]]` config parsing, and small-model Map waves are wired. Internal release gates write local reports under `reports/` for maintainers; seeded report gates and stronger scoring come next.

## Docs

- [Roadmap](docs/ROADMAP.md) · [路线图](docs/ROADMAP.zh.md)
- [Contributor handbook (AGENT.md)](AGENT.md) · [中文](docs/AGENT.zh.md)

Build the bilingual mdBook site locally:

```sh
make docs
```
