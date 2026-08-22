# burncloud-harness

`burncloud-harness` is **not** a general-purpose agent framework. It exists for one repository:

- `https://github.com/burncloud/burncloud`

Its job is to make coding agents produce safer, smaller, better-verified changes to BurnCloud by turning the repository's existing agent constitution into executable control.

## Design

BurnCloud already defines the engineering truth an agent must follow in `AGENTS.md` and `docs/agent/`. The harness therefore does not invent a second architecture. It enforces the existing one:

`DISCOVER -> UNDERSTAND -> TRACE -> CONTRACT -> PLAN -> CHANGE -> VERIFY -> INSPECT -> REPORT`

The project deliberately stays small and BurnCloud-specific:

1. Refuse to run outside a real `burncloud/burncloud` checkout.
2. Refuse to start a coding run from a dirty worktree.
3. Require an explicit task goal and path allowlist.
4. Read BurnCloud's current `TASK_ROUTER.md` and select likely source/evidence starting points for the task.
5. Read BurnCloud's current `INVARIANTS.md` and select candidate invariants from task context.
6. Inject those repository-derived hints plus BurnCloud's execution protocol into every agent attempt.
7. Detect and reject agent commits/history movement.
8. Inspect the actual git diff after the agent runs.
9. Fail closed on out-of-scope changes.
10. Derive mandatory verification from the BurnCloud areas that actually changed.
11. Feed verification failures back into the next agent attempt.
12. Store an append-only JSONL trajectory under the checkout's Git metadata, so harness data never becomes repository noise.

No graph engine, marketplace, generic plugin system, autonomous harness mutation, or multi-agent swarm is included.

## First BurnCloud intelligence layer

The harness should not carry a stale private copy of BurnCloud architecture. Its first intelligence layer is deliberately source-derived:

`task goal -> current TASK_ROUTER.md -> candidate source/evidence -> current INVARIANTS.md -> candidate invariants -> agent trace -> actual diff -> mandatory verification`

The harness only chooses where the agent should start looking. Current source code remains the authority for what is actually true.

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
cargo run -- explain --task examples/router-task.yaml
cargo run -- run --task examples/router-task.yaml
```

`explain` is intentionally read-only. It shows which current BurnCloud `TASK_ROUTER` rows and invariant IDs the harness selected before an agent is allowed to edit anything.

A successful `run` prints the changed paths and the trajectory file. Failed verification is automatically fed into the next attempt until `max_loops` is exhausted.

## BurnCloud-aware routing

The task router is not a second hard-coded BurnCloud architecture. The harness parses the target checkout's own `docs/agent/TASK_ROUTER.md`, scores its behavior rows against the task goal and coarse task area, and injects at most the strongest starting points.

These are explicitly treated as navigation hints, not runtime proof. The agent must still confirm the real execution path from current source before editing.

## BurnCloud-aware invariants

The harness parses invariant headings directly from the target checkout's `docs/agent/INVARIANTS.md`. It selects likely invariant families from the task area and routed behavior, for example router, billing, auth, database, runtime, and workspace invariants.

These are candidate invariants. The agent must verify their relevance and discover additional affected invariants from the real execution path.

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

The next useful layer is still not “more framework.” It is deeper BurnCloud control: compare selected invariants with the final changed paths, detect semantic diff risks, and analyze trajectories to find repeated BurnCloud failure patterns that deserve stronger deterministic rules.
