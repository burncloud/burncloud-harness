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
- stores append-only JSONL trajectories under Git metadata,
- analyzes those trajectories without automatically changing protected policy.

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

`explain` is read-only and shows the task-router starting points, candidate invariants, and declared scope before code editing begins.

`analyze` is also read-only. It summarizes recent trajectories and never changes harness policy.

## Post-change invariant control

Pre-change routing is only a prediction. After the agent edits code, the actual changed paths are stronger evidence.

For example, a router task may begin with only `INV-ROUTER-*`. If the final diff touches `crates/router/src/lib.rs`, the harness also treats `INV-BILLING-*` as impacted, forces another review attempt with those invariants visible, and then requires the billing/quota invariant suite before PASS.

The path-to-invariant map is intentionally small and BurnCloud-specific. It should grow only when BurnCloud source and invariant docs provide strong evidence for a stable ownership boundary.

## Final diff risk gate

The risk gate is deterministic. It scans the actual unified diff rather than asking another model whether a patch looks safe.

High-confidence blockers currently include:

- adding `#[ignore]`,
- adding `#[allow(clippy::unwrap_used)]`,
- deleting a dedicated BurnCloud invariant test file,
- removing protected security-boundary symbols without replacement in their documented ownership files.

Lower-confidence findings force one explicit review pass before verification. Current review findings include reduced assertions, new TODO/FIXME markers in runtime source, and reduced fail-closed/error constructs in sensitive paths.

Every finding is written to the trajectory.

## BurnCloud-aware verification

Verification is driven by changed paths plus active invariant IDs. Examples include:

- Rust changes -> `cargo fmt --check`
- router impact -> `cargo check -p burncloud-router`
- runtime impact -> `cargo check -p burncloud-server`
- auth/internal impact -> `cargo test -p burncloud-server --test security_invariants`
- billing impact -> `cargo test -p burncloud-router --test billing_invariants --test quota_tests`
- workspace dependency impact -> `cargo check --workspace`

Task-specific checks may be added, but built-in BurnCloud checks cannot be disabled by the task file.

## Trajectory analysis

Each run records task routing, invariant selection/expansion, actual changed paths, risk findings, verification results, retry feedback, and final outcome.

`analyze` reads the latest JSONL runs and reports:

- pass/fail/incomplete runs and success rate,
- average attempts per run,
- agent command failures,
- task-area distribution,
- repeated invariant expansions,
- final-diff risk codes,
- failed verification gates,
- scope-violation paths,
- repeated signals occurring at least three times.

Example shape:

```text
BurnCloud Harness Trajectory Analysis
runs=42 pass=35 fail=7 incomplete=0 success_rate=83.3% avg_attempts=2.10
agent_failures=3 scope_violation_events=2 parse_errors=0

Invariant expansions:
- 8x INV-BILLING-001

Final-diff risk signals:
- 5x ASSERTION_WEAKENING

Failed verification gates:
- 6x billing-invariants

Repeated signals worth harness review (>=3):
- invariant expansion: INV-BILLING-001 (8x)
- risk: ASSERTION_WEAKENING (5x)
- verification failure: billing-invariants (6x)
```

The analyzer intentionally stops there. Repetition is evidence for a human/harness-engineering decision; it is **not** permission for the worker agent to rewrite its own security policy.

## Evolution rule

A useful BurnCloud failure should move through this ladder only when evidence supports it:

`trajectory signal -> repeated pattern -> human review -> stronger routing/invariant/check/risk rule -> regression verification`

The next useful layer is better failure classification and evidence around those repeated signals, not broader orchestration.
