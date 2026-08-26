# MangoDisk - Design

## Product stance

The interface is an explorable storage landscape inside a restrained Black Glass
shell. Depth communicates directory hierarchy and selection consequence; it never
makes destructive cleanup feel playful or automatic.

## Token source

Consume `.context/design-system/tokens/` and set `data-bd-product="mangodisk"`.
Support exact dark/light geometry and all governed dark richness levels.

## Personality

- **Icon:** an anthropomorphic disk platter with a readable scanning “eye” and
  layered storage rings; avoid a trash can as the primary identity.
- **Density:** high-information analysis with calm review and confirmation zones.
- **Depth:** optical layers map storage hierarchy; keep base depth restrained.
- **Motion:** measured scan/reveal transitions; deletion and cleanup never use
  celebratory motion before verification.
- **Voice and type:** safety-first, with Mono for sizes, paths, rule IDs, and history.
- **Motif:** concentric capacity rings resolving into a navigable treemap.

## Components and states

Cover capacity overview, treemap/list, scan scope and progress, large files,
duplicates, cleanup rules, applications, protected paths, selection impact,
preflight, confirmation, execution, verification, partial failure, and history.
Storybook and HTML mockups require both themes, all richness levels, responsive
layouts, keyboard/focus, reduced motion, empty disks, permission denial, cancelled
scan, unavailable volume, and every destructive state. Tauri adoption waits for
explicit browser mockup approval.

## Accessibility

Treemaps require synchronized hierarchical tables and keyboard navigation. Sizes,
risks, and protection are textual, and destructive focus returns predictably.

## Exceptions to the shared foundation

None.
