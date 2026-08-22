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
- turns repeated hotspots into read-only, evidence-backed Harness improvement proposals,
- never lets trajectory analysis mutate protected policy automatically.

No graph engine, generic plugin system, autonomous harness mutation, or multi-agent swarm is included.

## Usage

```bash
cargo run -- doctor ../burncloud
cargo run -- explain --task examples/router-task.yaml
cargo run -- run --task examples/router-task.yaml
cargo run -- analyze ../burncloud --limit 100
cargo run -- recommend ../burncloud --limit 100 --min-count 3
```

`explain` is read-only and shows task-router starting points, candidate invariants, and declared scope before code editing begins.

`analyze` is read-only and reports run outcomes, structured failure classes, invariant/risk/check signals, and BurnCloud area/domain hotspots.

`recommend` is also read-only. It converts repeated area/domain failure hotspots into explicit improvement proposals with the supporting count. It cannot change prompts, permissions, invariant mappings, tests, or protected policy.

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

## Post-change invariant control

Pre-change routing is only a prediction. After the agent edits code, actual changed paths are stronger evidence.

For example, a router task may begin with only `INV-ROUTER-*`. If the final diff touches `crates/router/src/lib.rs`, the harness also treats `INV-BILLING-*` as impacted, forces another review attempt with those invariants visible, and then requires the billing/quota invariant suite before PASS.

The path-to-invariant map is intentionally small and BurnCloud-specific. It should grow only when BurnCloud source and invariant docs provide strong evidence for a stable ownership boundary.

## Final diff risk gate

The risk gate is deterministic. It scans the actual unified diff rather than asking another model whether a patch looks safe.

High-confidence blockers currently include adding `#[ignore]`, adding `#[allow(clippy::unwrap_used)]`, deleting a dedicated BurnCloud invariant test file, and removing protected security-boundary symbols without replacement in their documented ownership files.

Lower-confidence findings force one explicit review pass before verification. Current review findings include reduced assertions, new TODO/FIXME markers in runtime source, and reduced fail-closed/error constructs in sensitive paths.

## Structured failure taxonomy

Every failure emits a machine-readable `failure_recorded` trajectory event. Current classes are:

- `agent_command`
- `git_history`
- `scope_violation`
- `invariant_expansion`
- `no_change`
- `risk_block`
- `risk_review`
- `verification`
- `max_loops`

Natural-language feedback remains available to the next agent attempt, but Harness evolution no longer has to infer failure causes from prose.

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

`analyze` reports not only how often a failure occurs, but where it occurs:

```text
Failure classes:
- 9x verification
- 5x invariant_expansion

Failure hotspots by area:
- 7x router / verification
- 3x auth / risk_review

Failure hotspots by changed domain:
- 6x router / verification
- 4x database/router / invariant_expansion
```

Changed paths are collapsed into stable BurnCloud domains such as `router`, `server`, `client`, `database/router`, `database/billing`, `service/billing`, `service/channel`, `service/user`, `service/router-log`, `integration-tests`, `agent-docs`, and `workspace`. A failure touching several domains is counted once for each affected domain, not once per file.

## Evidence-backed proposals

`recommend` applies a minimum evidence threshold to the hotspot data. A proposal includes:

- priority,
- Harness layer that should be reviewed,
- whether the evidence came from task area or actual changed domain,
- hotspot context,
- failure class,
- occurrence count,
- a conservative suggested change.

Example:

```text
BurnCloud Harness Improvement Proposals
evidence_threshold=3 policy_mutation=disabled

1. [medium] Verification
   evidence: area / router -> 7x verification
   proposal: Move the most relevant BurnCloud check earlier for this hotspot or add a cheaper targeted preflight, while keeping the final invariant gate authoritative.
```

Recommendations are deliberately asymmetric: repeated `risk_block`, `scope_violation`, or `git_history` failures do **not** produce suggestions to weaken the guardrail. They recommend improving guidance/capability boundaries while preserving the hard block.

## Evolution rule

A useful BurnCloud failure should move through this ladder only when evidence supports it:

`structured failure -> repeated hotspot -> evidence-backed proposal -> human approval -> stronger routing/invariant/check/risk rule -> regression verification`

The worker agent still cannot rewrite the rules that control itself. The next step after this layer should be a human-approved proposal application workflow with explicit before/after regression evidence, not autonomous policy mutation.
