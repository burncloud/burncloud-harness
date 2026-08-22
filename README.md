# burncloud-harness

`burncloud-harness` is **not** a general-purpose agent framework. It exists for one repository:

- `https://github.com/burncloud/burncloud`

Its job is to make coding agents produce safer, smaller, better-verified changes to BurnCloud by turning BurnCloud's own engineering rules into executable control.

## Control loop

BurnCloud already defines the engineering truth in `AGENTS.md` and `docs/agent/`. The harness enforces that system instead of inventing a second architecture:

`DISCOVER -> UNDERSTAND -> TRACE -> CONTRACT -> PLAN -> CHANGE -> VERIFY -> INSPECT -> REPORT`

The current harness loop is:

`task goal -> TASK_ROUTER -> candidate invariants -> agent trace -> actual git diff -> scope -> invariant impact -> risk gate -> verification -> PASS / feedback`

Important properties:

- only runs against a real `burncloud/burncloud` checkout,
- starts coding runs only from a clean worktree,
- requires explicit allowed/avoid path scope,
- rejects agent commits and Git HEAD movement,
- includes untracked files in final-diff inspection,
- expands invariant impact from the actual diff rather than trusting only the initial task prediction,
- blocks high-confidence risk patterns and forces review for suspicious semantic weakening,
- derives mandatory checks from BurnCloud paths and active invariants,
- records failures as machine-readable classes in append-only trajectories,
- correlates those failures with BurnCloud task areas and actual changed-code domains,
- analyzes trajectories without automatically changing protected policy.

No graph engine, generic plugin system, autonomous harness mutation, or multi-agent swarm is included.

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

If source evidence shows that the root cause crosses the allowlist, the agent must report `NEED_SCOPE_EXPANSION` rather than silently widening the change.

## Usage

```bash
cargo run -- doctor ../burncloud
cargo run -- explain --task examples/router-task.yaml
cargo run -- run --task examples/router-task.yaml
cargo run -- analyze ../burncloud --limit 100
```

`explain` is read-only and shows task-router starting points, candidate invariants, and declared scope before code editing begins.

`analyze` is also read-only. It summarizes recent trajectories and never changes harness policy.

## Post-change invariant control

Pre-change routing is only a prediction. After the agent edits code, actual changed paths are stronger evidence.

For example, a router task may begin with only `INV-ROUTER-*`. If the final diff touches `crates/router/src/lib.rs`, the harness also treats `INV-BILLING-*` as impacted, forces another review attempt with those invariants visible, and then requires the billing/quota invariant suite before PASS.

The path-to-invariant map is intentionally small and BurnCloud-specific. It should grow only when BurnCloud source and invariant docs provide strong evidence for a stable ownership boundary.

## Final diff risk gate

The risk gate is deterministic. It scans the actual unified diff rather than asking another model whether a patch looks safe.

High-confidence blockers currently include adding `#[ignore]`, adding `#[allow(clippy::unwrap_used)]`, deleting a dedicated BurnCloud invariant test file, and removing protected security-boundary symbols without replacement in their documented ownership files.

Lower-confidence findings force one explicit review pass before verification. Current review findings include reduced assertions, new TODO/FIXME markers in runtime source, and reduced fail-closed/error constructs in sensitive paths.

## Structured failure taxonomy

Retry feedback is useful to the agent, but free-form text is poor data for Harness evolution. Every failure emits a separate `failure_recorded` trajectory event with a stable class:

- `agent_command` — the coding agent process failed,
- `git_history` — the agent changed HEAD or repository history,
- `scope_violation` — the actual diff escaped the declared allowlist,
- `invariant_expansion` — the actual diff introduced invariant impact not present in the pre-change contract,
- `no_change` — the agent completed without producing the required repository change,
- `risk_block` — deterministic final-diff policy found a blocking regression pattern,
- `risk_review` — the diff requires one explicit semantic review pass,
- `verification` — a mandatory BurnCloud check or invariant suite failed,
- `max_loops` — the task exhausted its bounded retry budget.

Natural-language feedback remains available to the next agent attempt, but analysis no longer has to infer the reason for failure from prose.

## BurnCloud-aware verification

Verification is driven by changed paths plus active invariant IDs. Examples include:

- Rust changes -> `cargo fmt --check`
- router impact -> `cargo check -p burncloud-router`
- runtime impact -> `cargo check -p burncloud-server`
- auth/internal impact -> `cargo test -p burncloud-server --test security_invariants`
- billing impact -> `cargo test -p burncloud-router --test billing_invariants --test quota_tests`
- workspace dependency impact -> `cargo check --workspace`

Task-specific checks may be added, but built-in BurnCloud checks cannot be disabled by the task file.

## Trajectory analysis and hotspots

Each run records task routing, invariant selection/expansion, actual changed paths, structured failure classes, risk findings, verification results, retry feedback, and final outcome.

`analyze` now reports not only how often a failure class occurs, but also where it occurs:

- `Failure hotspots by area` correlates the declared BurnCloud task area with failure class.
- `Failure hotspots by changed domain` correlates the actual diff domain with failure class.

Changed paths are collapsed into stable BurnCloud domains such as `router`, `server`, `client`, `database/router`, `database/billing`, `service/billing`, `service/channel`, `service/user`, `service/router-log`, `integration-tests`, `agent-docs`, and `workspace`. A failure touching several domains is counted once for each affected domain, not once per file.

Example shape:

```text
BurnCloud Harness Trajectory Analysis
runs=42 pass=35 fail=7 incomplete=0 success_rate=83.3% avg_attempts=2.10

Failure classes:
- 9x verification
- 5x invariant_expansion

Failure hotspots by area:
- 7x router / verification
- 3x auth / risk_review

Failure hotspots by changed domain:
- 6x router / verification
- 4x database/router / invariant_expansion

Invariant expansions:
- 8x INV-BILLING-001
```

The analyzer clears changed-path context at the start of every attempt so a failure that happens before a new diff is observed cannot be incorrectly attributed to the previous attempt's files.

The analyzer intentionally stops at evidence. Repetition is input to a human/harness-engineering decision; it is **not** permission for the worker agent to rewrite its own security policy.

## Evolution rule

A useful BurnCloud failure should move through this ladder only when evidence supports it:

`structured failure -> repeated hotspot -> human review -> stronger routing/invariant/check/risk rule -> regression verification`

The next useful layer is an evidence-backed proposal report: turn repeated hotspots into explicit suggested Harness changes with supporting counts, while still requiring human approval before any policy mutation.
