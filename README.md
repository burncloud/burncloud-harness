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
8. Inspect the actual git diff after the agent runs, including untracked files.
9. Fail closed on out-of-scope changes.
10. Recompute invariant impact from actual changed paths; if the diff touches an invariant family that was not in the pre-change contract, force another review loop with the expanded invariant set.
11. Run a deterministic final-diff risk gate before tests.
12. Derive mandatory verification from both actual BurnCloud paths and the active invariant set.
13. Feed invariant expansion, risk findings, and verification failures back into the next agent attempt.
14. Store an append-only JSONL trajectory under the checkout's Git metadata, so harness data never becomes repository noise.

No graph engine, marketplace, generic plugin system, autonomous harness mutation, or multi-agent swarm is included.

## BurnCloud control loop

The harness deliberately distinguishes prediction from reality:

`task goal -> current TASK_ROUTER.md -> candidate invariants -> agent trace -> actual git diff -> invariant impact -> risk gate -> verification -> PASS / feedback`

Pre-change routing only predicts what matters. After the agent edits code, the actual diff becomes the stronger source of evidence.

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

A successful `run` prints the changed paths and the trajectory file. Invariant expansion, risk findings, and failed verification are automatically fed into later attempts until `max_loops` is exhausted.

## BurnCloud-aware routing and invariants

The harness does not carry a private copy of BurnCloud architecture. It reads the target checkout's own `TASK_ROUTER.md` and `INVARIANTS.md`, then reassesses the real diff against a deliberately small set of high-value ownership boundaries.

Current post-change mappings cover runtime startup, router composition, auth/internal control plane, billing/quota settlement, database placeholder behavior, and workspace dependency invariants. The map should only grow when BurnCloud source/docs provide strong evidence for a stable boundary.

## Final diff risk gate

The risk gate is deterministic. It does not ask a second model whether the patch “looks safe.” It scans the actual unified diff for a small set of high-signal failure modes.

Blocking findings must be removed before the run can pass. Current blockers include:

- adding `#[ignore]`,
- adding `#[allow(clippy::unwrap_used)]`,
- deleting a dedicated BurnCloud invariant test file,
- removing protected security-boundary symbols without replacement in the same file, including `security_boundary_middleware`, `auth_middleware`, `admin_middleware`, `BURNCLOUD_INTERNAL_SECRET`, and `X-Internal-Secret` in their documented ownership paths.

Review findings force one additional agent review pass. If the same finding remains unchanged after that pass, the harness treats it as reviewed and continues to verification. Current review findings include:

- reducing assertions in tests,
- adding TODO/FIXME markers to runtime source,
- reducing fail-closed/error constructs in sensitive router/server/token paths.

Every finding is recorded in the trajectory. This distinction keeps obviously dangerous changes hard-blocked while allowing intentional refactors to proceed after an explicit review loop.

## BurnCloud-aware verification

Verification is driven by both the changed paths and active invariant IDs. Examples:

- Rust changes -> `cargo fmt --check`
- router impact -> `cargo check -p burncloud-router`
- runtime impact -> `cargo check -p burncloud-server`
- auth/internal impact -> `cargo test -p burncloud-server --test security_invariants`
- billing impact -> `cargo test -p burncloud-router --test billing_invariants --test quota_tests`
- workspace dependency impact -> `cargo check --workspace`

Task-specific checks may be added, but built-in BurnCloud checks cannot be disabled by the task file.

## Trajectory

Every run records:

- task routing,
- candidate invariants,
- actual changed paths,
- post-change invariant expansion,
- final-diff risk findings,
- verification commands/results,
- retry feedback,
- final result.

That data is the basis for later Harness evolution: repeated failure patterns can be promoted from agent guidance into stronger deterministic checks.

## What evolves next

The next useful layer is trajectory analysis, not more orchestration: summarize repeated BurnCloud failure classes across runs and identify which ones are stable enough to become new hard rules, invariant mappings, or verification gates.
