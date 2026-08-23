# BurnCloud UI Source Migration Tasks

These tasks migrate the active UI from `rustburn/burncloud-ui` into the Rust/Dioxus client in `burncloud/burncloud`.

## Source pin

- Repository: `rustburn/burncloud-ui`
- Required source commit: `ce4fa9d2e79928a388bffa363a1eec77f6998900`
- The active route graph in `src/App.tsx` is the source implementation truth.
- Unused legacy/generic React pages are not migration targets.

## Required local layout

Run the harness from the `burncloud-harness` repository with all three repositories checked out as siblings:

```text
workspace/
├── burncloud-harness/
├── burncloud/
└── burncloud-ui/
```

Before starting the migration, check out the source repository at the pinned commit:

```bash
git -C ../burncloud-ui checkout ce4fa9d2e79928a388bffa363a1eec77f6998900
```

The task loader intentionally resolves source React files as read-only `context_files`. If `../burncloud-ui` is missing, a migration task must fail before the coding agent starts.

## Truth priority

Migration has three independent kinds of truth:

1. Approved `burncloud-harness/docs/ui/**` product/page contracts define product semantics and role boundaries.
2. Current `burncloud/burncloud` runtime/API/data defines which values may be claimed as real.
3. Pinned `burncloud-ui` defines presentation truth for source-port tasks: shell, role navigation, visual topology, component hierarchy, density, spacing, and interaction placement.

Read `docs/ui/source-migration-fidelity.md` before running a Golden Page migration.

A missing runtime value may change `$14.28` to `Unavailable`; it does not authorize changing a four-metric source layout into a different legacy dashboard. Truthful fallback content must preserve the source page's recognizable geometry.

## Change budget

A one-page migration is not a frontend rewrite.

UI tasks should declare `scope.max_changed_files`. The Harness fails closed when the real Git diff exceeds that budget even if every changed file otherwise matches an allowed glob.

If a task needs more files than its budget, stop and explicitly expand the task contract. Do not use `crates/client/**` as permission to migrate unrelated pages, rewrite route aliases, edit verification scripts, or change dependencies.

Buyer Overview is intentionally stricter than the initial test run: it has an explicit eight-file allowlist and an eight-file hard budget.

## i18n

The source UI currently supports:

- `en`
- `zh`
- `zh-TW`
- `ja`

The Rust/Dioxus migration must preserve these language semantics and avoid page-local hard-coded user-facing copy.

## Ordered execution

Do not run migration tasks in parallel against the same BurnCloud checkout. Each page task may make only the shared i18n/layout/component changes required by that page.

### Phase 1 — Golden Pages

Run these first, in this exact order:

1. `buyer-overview.yaml`
2. `02-supplier-overview.yaml`
3. `03-admin-overview.yaml`
4. `04-buyer-marketplace.yaml`
5. `05-supplier-resources.yaml`
6. `06-admin-capacity.yaml`

These six pages establish the shared visual hierarchy, role navigation, i18n primitives, component patterns, status language, CTA hierarchy, and density for the remaining migration.

Do not move to page 2 merely because page 1 compiles. The source and Rust page must be recognizable as the same approved page at the major-layout level.

### Phase 2 — Remaining authenticated pages

After the Golden Pages are stable, continue one page per Harness task in this order:

7. Buyer Playground
8. Buyer API Keys
9. Buyer Usage
10. Buyer Billing
11. Buyer Logs
12. Supplier Deployments
13. Supplier Earnings
14. Supplier Settlements
15. Supplier Reliability
16. Supplier Settings
17. Admin Supply
18. Admin Demand
19. Admin Models
20. Admin Revenue
21. Admin Settlements
22. Admin Suppliers
23. Admin Customers
24. Admin Operations
25. Admin Settings

### Phase 3 — Public/auth pages

26. Home / Landing
27. Login
28. Register

The remaining tasks should be generated only after the Golden Pages prove the shared Rust/Dioxus patterns. This prevents later pages from locking in a bad shared component or i18n design before Harness evidence exists.

## Formal Harness test

Start with Buyer Overview:

```bash
cargo run -- run --task tasks/ui/buyer-overview.yaml
```

Harness run state is stored in the target BurnCloud repository, not in the harness repository:

```text
../burncloud/.git/burncloud-harness/runs/<run_id>/
```

In a second terminal, observe the target workspace. `../burncloud` is the default, so the short command works from the sibling layout above:

```bash
cargo run -- tui
```

The explicit equivalent is:

```bash
cargo run -- tui --workspace ../burncloud
```

After the run:

```bash
cargo run -- tui --list
cargo run -- explain-run --run <run_id>
```

Both observer commands default to `../burncloud`. Use `--workspace <path>` if the BurnCloud checkout is elsewhere.

Review the evidence bundle under:

```text
../burncloud/.git/burncloud-harness/runs/<run_id>/
├── task.yaml
├── events.jsonl
├── trajectory.jsonl
├── diff.patch
└── summary.json
```

## Golden Page acceptance

For each Golden Page, review these independently:

```text
1. Product semantics / role boundary
2. Runtime truthfulness
3. Source shell and navigation parity
4. Major page-section topology parity
5. i18n behavior
6. Change budget / diff focus
7. Compile and functional checks
```

A PASS that only proves items 1, 2, and 7 is not enough for source migration.

Only move to the next task when the current page passes its product/page contract, source-migration fidelity contract, change budget, and verification gates.
