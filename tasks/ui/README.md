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

1. Approved `burncloud-harness/docs/ui/**` product/page contracts.
2. Current `burncloud/burncloud` runtime, API, security, and data truth.
3. Pinned `burncloud-ui` implementation as the visual/content/interaction reference.

The source React UI should be reproduced closely, but demo/mock values must never be promoted into fake runtime facts.

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

Only move to the next task when the current page passes its product/page contract and verification gates.
