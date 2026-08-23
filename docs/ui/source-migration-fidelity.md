---
doc_id: ui.source-migration-fidelity
doc_type: migration-standard
truth: target
status: approved
version: 1.1
parent:
  - docs/ui/product-standard.md
  - docs/ui/agent-execution.md
---

# BurnCloud Source UI Migration Fidelity v1.1

## 1. Purpose

This document governs one-way migration from the approved `rustburn/burncloud-ui` implementation into the Rust/Dioxus client in `burncloud/burncloud`.

For these migration tasks, success is not "the page compiles" and not "the page contains approximately the same information".

Success means:

> The Rust page preserves the approved source page's recognizable visual topology and interaction hierarchy while replacing demo facts with truthful BurnCloud runtime states.

## 2. Three Independent Truths

Migration must keep these separate:

1. Product truth — `burncloud-harness/docs/ui/**` defines role boundaries, semantics, product behavior, and truthful-state rules.
2. Runtime truth — current `burncloud/burncloud` APIs and data determine what can be claimed as real.
3. Presentation truth — the pinned `rustburn/burncloud-ui` page and shared layout define the visual composition, density, navigation shape, component hierarchy, spacing, and interaction placement to reproduce.

Runtime truth may change displayed values. It must not be used as an excuse to discard presentation truth.

## 3. Unknown Data Must Preserve Geometry

When a source value is demo/mock and BurnCloud has no real equivalent, the Rust implementation must keep the same visual container and hierarchy.

Examples:

```text
Source metric card: Today Spend = $14.28
Rust without reliable billing data: Today Spend = Unavailable
```

The metric card remains a metric card in the same position.

```text
Source Models in Use table has columns and rows
Rust has no confirmed model-usage rows
```

The section, card, header, table/skeleton geometry, and empty state remain in the same source location. Do not replace the whole section with a generic full-width warning block.

Truthful states change content, not the page's identity.

## 4. Visual Topology Is a Contract

For a source-port task, the following are required unless an approved product contract explicitly says otherwise:

- same role-specific application shell
- same sidebar role and navigation grouping
- same top header/search/action placement
- same page header hierarchy
- same primary CTA hierarchy
- same major section order
- same number and order of primary metric slots
- same table/card/list form for major content areas
- comparable desktop density and whitespace
- comparable border radius, surface treatment, typography hierarchy, and status treatment
- responsive behavior that preserves the source information priority

A migration that keeps labels but substitutes a different legacy shell or a different page composition fails fidelity.

## 5. Buyer Overview Required Landmarks

The Buyer Overview golden page must be visually recognizable from the pinned source implementation before it can pass.

Required landmarks:

```text
Buyer role shell
├── BurnCloud role/workspace switcher
├── Buyer workflow/navigation
├── top search
├── Autopilot/status area
├── language switcher
└── user/account area

Buyer Overview
├── page title + concise subtitle
├── conclusion/status strip
├── actions near page header
├── four primary metrics in fixed order
│   ├── Today Spend
│   ├── Balance
│   ├── API Availability
│   └── Tokens Today
├── Needs Attention only when warranted
├── Models in Use card/table area
└── Recent Activity card/list area
```

If data is unavailable, these landmarks still exist with truthful state content.

## 6. Do Not Fall Back to the Legacy Console Shell

During migration, existing Rust UI may contain older generic Console navigation or System Overview patterns.

Those are current-source implementation details, not presentation authority for a source-port task.

Do not preserve legacy shell structure merely because it already compiles.

For Buyer migration specifically, do not keep a generic navigation such as:

```text
Providers
Models
Routes
Evaluation
Customers
Team
```

as the Buyer primary navigation when the approved source and information architecture require Buyer-oriented navigation.

## 7. Smallest Coherent Change

Source fidelity does not authorize a frontend rewrite.

One page task must:

- change only files required by that page and shared shell primitives it directly needs
- avoid migrating later pages early
- avoid touching unrelated functional pages to make checks green
- avoid broad route alias rewrites unrelated to the page
- avoid changing client dependencies unless the task cannot be implemented with the current stack

Task `scope.max_changed_files` is a hard engineering budget, not a target to fill.

If the page genuinely needs more files than the declared budget, fail and request an explicit scope expansion instead of silently widening the change.

## 8. Fidelity Review Before PASS

Before declaring completion, compare source and target in this order:

1. Application shell
2. Navigation role and grouping
3. Page header and actions
4. Major section order
5. Primary metric geometry
6. Main table/list/card geometry
7. Empty/loading/error placement
8. Typography and spacing hierarchy
9. i18n behavior
10. Responsive priority

The report must explicitly list any remaining mismatch.

Statements such as "implemented the overview" or "product check passed" are not sufficient evidence.

## 9. Convergence Protocol

Source-port work must converge from large structural differences toward small visual differences. The Agent must not repeatedly rediscover the whole UI on every Harness attempt.

### 9.1 Initial pass

The first attempt uses this order:

```text
SOURCE PAGE + SHARED LAYOUT
        ↓
CURRENT TARGET PAGE + TARGET LAYOUT/CSS
        ↓
DELTA LIST
        ↓
STRUCTURE
        ↓
CONTENT/TRUTHFUL STATE
        ↓
SPACING / TYPOGRAPHY
        ↓
VERIFY
```

Before the first edit, identify the concrete differences between source and target. Once source ownership and target ownership are known, stop broad repository exploration and implement those differences.

Task context has two reading levels:

- **Primary context** — page contract, source page, source shared layout, source-fidelity standard, and directly relevant design/IA documents. Read these first.
- **Supporting context** — locale files, shared component libraries, additional product documents, data fixtures, and other references. Consult these only when a specific delta requires them.

A long context list is not a requirement to serially read every file before every edit.

### 9.2 Revision pass

Any later attempt that already has a real diff or Harness feedback is a revision pass.

The Agent must:

1. inspect the current Git diff first;
2. read the previous Harness/human feedback;
3. convert feedback into a small ordered delta list;
4. preserve already-correct sections;
5. reopen only the source/contract evidence required by the active delta;
6. make the smallest correction at the narrowest layer;
7. verify the corrected delta before moving on.

Revision must not restart from a blank mental model or broadly rewrite an already recognizable page.

Correction layer priority:

```text
spacing / visual mismatch
    -> local CSS/layout

missing landmark / wrong hierarchy
    -> page/component structure

truthful-state mismatch
    -> data/content wiring

only widen further when evidence requires it
```

This ordering exists to prevent a small spacing rejection from turning into another full-page rewrite.

### 9.3 Completion discipline

A later attempt may inspect only the previously rejected areas plus their immediate layout dependencies. It does not need to re-prove unrelated sections that the current diff did not disturb.

However, every attempt must still obey BurnCloud scope, invariant, risk, and verification gates. Convergence reduces repeated exploration; it does not weaken quality control.

## 10. Failure Examples

The following must be treated as migration failure even when Rust compiles:

- Buyer source uses Buyer navigation but target still shows the legacy generic Console sidebar.
- Source has four primary metric slots but target removes their card/metric structure because values are unknown.
- Source has `Models in Use` and `Recent Activity` as major sections but target replaces them with generic unavailable blocks.
- Agent changes many unrelated pages during a one-page migration.
- Page uses correct text but visibly different information hierarchy.
- Mock values are copied into production as if real.
- A revision pass rereads broad context and rewrites already-correct regions instead of fixing the rejected delta.
- A local spacing mismatch causes unrelated Rust component or routing changes without evidence.

## 11. Completion Rule

A source-port page is complete only when all of these are true:

```text
Product contract valid
AND runtime claims truthful
AND source visual topology recognizable
AND role shell/navigation correct
AND change budget respected
AND compile/functional gates pass
AND remaining parity gaps are explicitly reported
```

Build green alone can never satisfy this rule.
