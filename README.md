# burncloud-harness

A Rust-first agent harness for running coding agents inside explicit goals, boundaries, checks, and observable feedback loops.

The project is intentionally starting small: a deterministic task loop, scope policy, checks, and append-only trajectory records come before graph orchestration or autonomous harness mutation.

## Direction

`goal -> agent -> action -> checks -> feedback -> retry -> trajectory`

A separate harness-improvement loop will later analyze trajectories and propose changes, but protected policies remain human-controlled.
