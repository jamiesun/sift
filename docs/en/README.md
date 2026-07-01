# sift

> English | [中文](../README.zh.md)

Cost-controlled open-source project auditor: **tiered funnel + compute mismatch + ReACT scheduling**. Before adopting a dependency, get a file/line-level risk ledger without force-feeding tens of thousands of lines into a frontier model.

- Grunt work (structure extraction / deterministic coarse filtering) -> tree-sitter + local rules
- Logic convergence -> frontier large model, orchestrated by a ReACT state machine over deterministic findings
- Single binary, zero-config; audits a whole project or a single module; sift must pass its internal release gates

See [Roadmap](../ROADMAP.md) for full design.

## Usage

```sh
sift ./repo --scan-only        # scan layer only (no key needed)
sift ./repo --agent-gate       # deterministic pre-run gate (no key needed)
sift ./repo --agent-gate --format json
sift ./repo --benchmark        # scan/model budget telemetry JSON (no key needed)
sift github owner/repo         # safe GitHub intake, defaults to --agent-gate
sift github owner/repo --ref main --scan-only
sift eval-corpus               # run the checked-in repo-intake precision corpus
sift ./repo --module src        # audit a submodule
SIFT_API_KEY=<KEY> sift ./repo  # full pipeline
sift ./repo --api-key-file ~/.sift/key
sift ./repo --report-language zh # request a Simplified Chinese Markdown report
sift ./repo --debug              # print extra diagnostics to stderr
sift doctor                    # check config, key_env, and endpoint/key mismatches
```

`--agent-gate` is a local, deterministic repo-intake gate for agents and wrapper scripts. It writes only this stable contract to stdout:

```text
VERDICT: ACCEPT | CAUTION | REJECT | INCOMPLETE
WHY:
- <top evidence>
BLOCKERS:
- <file:line evidence or coverage blocker>
SAFE_TO_AGENT_RUN: yes | no
```

The command exits `0` only when `SAFE_TO_AGENT_RUN: yes`; `CAUTION`, `REJECT`, and `INCOMPLETE` exit non-zero so callers can stop before setup, install, build, or run steps.

Use `--format json` with `--agent-gate` for automation. The JSON contract contains `verdict`, `safe_to_agent_run`, `exit_reason`, `coverage`, `findings`, `blockers`, artifact inventory, truncation details, and policy actions.

The deterministic supply-chain layer currently flags npm install lifecycle scripts, manifest/lockfile reproducibility gaps, git/path/http dependency sources, Rust `build.rs` command boundaries, shell/Dockerfile download-execute patterns, base64 decode-to-execute flows, GitHub Actions permission/trigger risk, secrets coupled to shell execution, unpinned GitHub Actions, Dockerfile root/remote repository patterns, and suspicious binary/archive artifacts.

`sift github` accepts `owner/repo` or `https://github.com/owner/repo`, fetches a temporary checkout with `git`, resolves the commit SHA, then runs the local scan/gate/benchmark pipeline against that checkout. It never runs repository code, package manager commands, build scripts, hooks, install commands, or submodules. The checkout is inspected for file/byte limits, `.gitmodules`, and Git LFS indicators before scanning. Temporary checkouts are removed by default; use `--keep-checkout` only when you need to inspect the fetched tree.

Project-local policy lives in `sift-policy.toml`. It supports `max_candidate_files`, `[[allowlist]]`, `[[denylist]]`, and `[[severity_override]]` entries keyed by `path`, `rule`, `severity`, and `reason`; applied policy decisions are shown in text and JSON gate output.

The current full-audit path does not call small-model Map by default. It converges from the deterministic ledger with the configured large model, while the small-model Map implementation remains an experimental diagnostic path.

## Supported Languages

The scan layer currently dehydrates Rust, Python, Go, JavaScript, TypeScript/TSX, HTML, CSS, Zig, Bash-compatible shell files (`.sh`, `.bash`, `.zsh`), Dart, Kotlin, Java, C/C++, C#, PHP, Swift, Ruby, SQL, Dockerfile/Containerfile, YAML, HCL/Terraform, Vue, Svelte, `package.json`, common package manifests/lockfiles, Makefile, and Markdown install snippets.

## Install

```sh
make ci
make install
```

## More

- [Roadmap](../ROADMAP.md)
- [Contributor Handbook](AGENT.md)
