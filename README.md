# burncloud-harness

`burncloud-harness` is **not** a general-purpose agent framework. It exists for one repository:

- `https://github.com/burncloud/burncloud`

Its job is to make coding agents produce safer, smaller, better-verified changes to BurnCloud by turning the repository's existing agent constitution into executable control.

## v0.1 design

BurnCloud already defines the engineering truth an agent must follow in `AGENTS.md` and `docs/agent/`. The harness therefore does not invent a second architecture. It enforces the existing one:

`DISCOVER -> UNDERSTAND -> TRACE -> CONTRACT -> PLAN -> CHANGE -> VERIFY -> INSPECT -> REPORT`

The first version deliberately stays small:

1. Refuse to run outside a real `burncloud/burncloud` checkout.
2. Refuse to start from a dirty worktree.
3. Require an explicit task goal and path allowlist.
4. Inject BurnCloud's repository bootstrap and execution protocol into every agent attempt.
5. Detect and reject agent commits/history movement.
6. Inspect the actual git diff after the agent runs.
7. Fail closed on out-of-scope changes.
8. Derive mandatory verification from the BurnCloud areas that actually changed.
9. Feed verification failures back into the next agent attempt.
10. Store an append-only JSONL trajectory under the checkout's Git metadata, so harness data never becomes repository noise.

No graph engine, marketplace, generic plugin system, autonomous harness mutation, or multi-agent swarm is included in v0.1.

## Example task

```yaml
name: router-fallback-fix
goal: Preserve provider fallback when the first upstream returns a retryable failure.
workspace: ../burncloud
area: router
max_loops: 3

scope:
  allowed:
    - crates/router/**
    - crates/tests/tests/api/**
  avoid:
    - crates/service/crates/billing/**
    - crates/database/**

agent:
  program: codex
  args:
    - exec
    - --full-auto
  append_prompt: true

extra_checks: []
```

The allowlist is a hard boundary. `avoid` further narrows it. If source evidence shows that the root cause crosses the boundary, the agent is instructed to stop and report `NEED_SCOPE_EXPANSION` rather than silently widening the change.

## Usage

```bash
cargo run -- doctor ../burncloud
cargo run -- run --task examples/router-task.yaml
```

A successful run prints the changed paths and the trajectory file. Failed verification is automatically fed into the next attempt until `max_loops` is exhausted.

## BurnCloud-aware verification

The harness reads the changed paths and adds repository-specific checks. Examples:

- Rust changes -> `cargo fmt --check`
- `crates/router/**` -> `cargo check -p burncloud-router`
- `crates/server/**` -> `cargo check -p burncloud-server`
- server changes -> `security_invariants`
- router / router-database / router-log changes -> `billing_invariants` + `quota_tests`
- root `Cargo.toml` / `Cargo.lock` -> `cargo check --workspace`

Task-specific checks may be added, but built-in BurnCloud checks cannot be disabled by the task file.

## What evolves next

The next useful layer is not “more framework.” It is better BurnCloud control: richer task routing, invariant-to-path mapping, deterministic final-diff inspection, and trajectory analysis that tells us which BurnCloud failure patterns should become stronger harness rules.
