# burncloud-harness

`burncloud-harness` is **not** a general-purpose agent framework. It exists for one repository:

- `https://github.com/burncloud/burncloud`

Its job is to make coding agents produce safer, smaller, better-verified changes to BurnCloud by turning BurnCloud's own engineering rules into executable control.

## Control loop

BurnCloud already defines the engineering truth in `AGENTS.md` and `docs/agent/`. The harness enforces that system instead of inventing a second architecture:

`DISCOVER -> UNDERSTAND -> TRACE -> CONTRACT -> PLAN -> CHANGE -> VERIFY -> INSPECT -> REPORT`

The current Harness + Loop is:

`task goal -> TASK_ROUTER -> candidate invariants -> agent -> actual git diff -> scope -> invariant impact -> risk gate -> verification -> PASS / feedback -> next loop`

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
- can render the same control state in an interactive Ratatui console,
- never lets trajectory analysis mutate protected policy automatically.

No graph engine, generic plugin system, autonomous harness mutation, or multi-agent swarm is included.

## Usage

```bash
cargo run -- doctor ../burncloud
cargo run -- explain --task examples/router-task.yaml
cargo run -- run --task examples/router-task.yaml
cargo run -- run --task examples/router-task.yaml --resume
cargo run -- run --task examples/router-task.yaml --resume --verify-existing
cargo run -- run --task examples/router-task.yaml --tui
cargo run -- analyze ../burncloud --limit 100
cargo run -- recommend ../burncloud --limit 100 --min-count 3
```

`explain` is read-only and shows task-router starting points, candidate invariants, and declared scope before code editing begins.

`run --tui` executes the **same Harness and the same Loop** as normal `run`. Ratatui is only an observer/control-plane UI; it does not receive extra write permissions and it does not replace any gate.

`run --resume` continues changes left by an interrupted Harness run. It fails closed unless every existing changed path is inside the task allowlist and outside the avoid list; the resumed diff still passes the normal invariant, risk, and verification gates.

`run --resume --verify-existing` skips another coding-agent invocation and runs one deterministic gate pass over operator-approved resumed changes. Use it only after reviewing the interrupted agent's report and diff; scope, Git history, invariant impact, risk, and mandatory checks remain enforced.

`analyze` is read-only and reports run outcomes, structured failure classes, invariant/risk/check signals, and BurnCloud area/domain hotspots.

`recommend` is also read-only. It converts repeated area/domain failure hotspots into explicit improvement proposals with the supporting count. It cannot change prompts, permissions, invariant mappings, tests, or protected policy.

## Codex invocation

For current Codex CLI versions, BurnCloud tasks should use an explicit workspace-write sandbox:

```yaml
agent:
  program: codex
  args:
    - exec
    - --sandbox
    - workspace-write
  append_prompt: true
```

Older task files that still contain `--full-auto` are normalized by `burncloud-harness` to `--sandbox workspace-write` when the configured agent program is Codex. This keeps historical task files working while making the effective permission mode explicit.

Task-owned reference documents that live outside the BurnCloud checkout must be declared explicitly. Paths are resolved relative to the task YAML and exposed to the agent as read-only context:

```yaml
context_files:
  - ../../docs/ui/product-standard.md
  - ../../docs/ui/page-contracts/buyer-overview.md
```

The Codex sandbox is **not** the BurnCloud scope boundary. Codex may write inside the checkout, while `burncloud-harness` independently inspects the real Git diff and rejects changes outside the task allowlist.

## Ratatui Harness Console

The console exists to improve the developer's mental model, not merely to make logs prettier.

It deliberately shows the distinction:

```text
HARNESS = boundaries + context + deterministic feedback
LOOP    = attempt -> evidence -> feedback -> retry
```

The screen contains five conceptual views:

1. **Task Contract** — task, goal, BurnCloud area, current attempt, current phase, and why the Harness is in that phase.
2. **Hard Boundary** — explicit `ALLOW` and `DENY` paths plus any real scope violation detected from Git.
3. **Evidence-driven Loop** — `AGENT -> SCOPE -> INVARIANTS -> RISK -> VERIFY -> FEEDBACK`, with the current phase highlighted and every retry reason preserved.
4. **Actual Reality** — active invariants, routing evidence, actual changed paths, risk findings, and mandatory checks.
5. **Mental Model** — continuously reminds the operator that the Harness owns boundaries while Codex acts inside them; failed evidence becomes the next Loop input.

A typical loop becomes visible as:

```text
Attempt 1 / 3
AGENT -> SCOPE -> INVARIANTS
                     |
                     +-- INV-BILLING-001 discovered from actual diff

Feedback: invariant_expansion
                     |
                     v
Attempt 2 / 3
AGENT -> SCOPE -> INVARIANTS -> RISK -> VERIFY
                                          |
                                          +-- billing-invariants FAIL

Feedback: verification
                     |
                     v
Attempt 3 / 3
...
```

The console waits on the final PASS/STOPPED screen so a developer can inspect the result, then closes with `Enter`, `q`, `Esc`, or `Ctrl-C`.

## Observer boundary

The Ratatui integration is intentionally implemented as an observer over the runner. The runner emits structured state transitions such as:

- prepared task contract,
- current Harness phase,
- actual changed paths and violations,
- invariant expansion,
- risk findings,
- check start/result,
- structured failure class,
- final result and trajectory path.

Headless `run` uses a no-op observer. `run --tui` uses the Ratatui observer. Both paths call the same `run_with_observer` core, so visualization cannot silently fork the security behavior.

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
    - --sandbox
    - workspace-write
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

- Rust changes -> `git diff --check HEAD -- <changed-rust-files>` (baseline-aware changed-line whitespace gate)
- client impact -> `cargo check -p burncloud-client`
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

`recommend` applies a minimum evidence threshold to hotspot data. A proposal includes priority, Harness layer, evidence source, hotspot context, failure class, occurrence count, and a conservative suggested change.

Recommendations are deliberately asymmetric: repeated `risk_block`, `scope_violation`, or `git_history` failures do **not** produce suggestions to weaken the guardrail. They recommend improving guidance/capability boundaries while preserving the hard block.

## Evolution rule

A useful BurnCloud failure should move through this ladder only when evidence supports it:

`structured failure -> repeated hotspot -> evidence-backed proposal -> human approval -> stronger routing/invariant/check/risk rule -> regression verification`

The worker agent still cannot rewrite the rules that control itself. Ratatui makes those rules and loops visible; it does not relax them.
