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
10. Recompute invariant impact from the actual changed paths; if the diff touches an invariant family that was not in the pre-change contract, force another review loop with the expanded invariant set.
11. Derive mandatory verification from both actual BurnCloud paths and the active invariant set.
12. Feed verification failures back into the next agent attempt.
13. Store an append-only JSONL trajectory under the checkout's Git metadata, so harness data never becomes repository noise.

No graph engine, marketplace, generic plugin system, autonomous harness mutation, or multi-agent swarm is included.

## BurnCloud control loop

The harness deliberately distinguishes prediction from reality:

`task goal -> current TASK_ROUTER.md -> candidate invariants -> agent trace -> actual git diff -> invariant impact -> verification -> PASS / feedback`

Pre-change routing only predicts what matters. After the agent edits code, the actual changed paths can expand the invariant contract. That expansion cannot be silently ignored.

Example:

```text
Goal: fix router retry
Pre-change invariants: INV-ROUTER-*

Actual diff:
  crates/router/src/lib.rs

Post-change impact:
  INV-ROUTER-*
  INV-BILLING-*

Harness response:
  require another agent review loop with billing invariants visible
  then run router compilation + billing/quota invariant tests
```

This is intentional: BurnCloud's real diff is stronger evidence than the harness's initial guess.

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

A successful `run` prints the changed paths and the trajectory file. Failed verification or newly discovered invariant impact is automatically fed into the next attempt until `max_loops` is exhausted.

## BurnCloud-aware routing

The task router is not a second hard-coded BurnCloud architecture. The harness parses the target checkout's own `docs/agent/TASK_ROUTER.md`, scores its behavior rows against the task goal and coarse task area, and injects at most the strongest starting points.

These are explicitly treated as navigation hints, not runtime proof. The agent must still confirm the real execution path from current source before editing.

## BurnCloud-aware invariants

The harness parses invariant headings directly from the target checkout's `docs/agent/INVARIANTS.md`. It selects likely invariant families from the task area and routed behavior, then reassesses the actual diff against high-value BurnCloud ownership paths.

Current post-change mappings intentionally cover the strongest documented invariant boundaries first:

- `src/main.rs` -> runtime startup invariants
- `crates/server/src/lib.rs` -> runtime, router composition, and auth boundary invariants
- `crates/server/src/api/**` -> auth invariants
- `crates/server/src/api/auth.rs` -> auth + internal-control invariants
- `crates/router/src/**` -> router invariants
- `crates/router/src/lib.rs` -> router + billing invariants
- router token/quota implementation and invariant tests -> billing invariants
- `crates/database/src/placeholder.rs` -> database placeholder invariant
- root `Cargo.toml` / `Cargo.lock` -> workspace dependency invariants

This map should only grow when BurnCloud source/docs provide strong evidence for a stable boundary.

## BurnCloud-aware verification

Verification is now driven by both the changed paths and active invariant IDs. Examples:

- Rust changes -> `cargo fmt --check`
- router impact -> `cargo check -p burncloud-router`
- runtime impact -> `cargo check -p burncloud-server`
- auth/internal impact -> `cargo test -p burncloud-server --test security_invariants`
- billing impact -> `cargo test -p burncloud-router --test billing_invariants --test quota_tests`
- workspace dependency impact -> `cargo check --workspace`

Task-specific checks may be added, but built-in BurnCloud checks cannot be disabled by the task file.

## Trajectory

Every run records the pre-change route/invariant selection and the post-change invariant impact. This gives later harness evolution a factual dataset for questions such as:

- which tasks repeatedly expand beyond their predicted invariant set,
- which BurnCloud files create the most cross-domain impact,
- which invariant gates catch regressions most often,
- where task routing or scope definitions should become stronger.

## What evolves next

The next useful layer is still not “more framework.” It is deterministic final-diff risk inspection: detect suspicious semantic changes such as weakened assertions, removed authorization checks, deleted error handling, or broad refactors that are inconsistent with the declared task contract, then record those failures as trajectory data.
