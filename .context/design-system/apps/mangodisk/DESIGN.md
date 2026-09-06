---
name: "MangoDisk"
description: "MangoDisk: concentric capacity rings resolving into a navigable treemap"
colors:
  canvas-dark: "#101316"
  shell-dark: "#171A1D"
  raised-dark: "#1D2125"
  ink-dark: "#E6E8EB"
  canvas-light: "#ECEDEF"
  shell-light: "#F8F9FB"
  raised-light: "#FBFCFE"
  ink-light: "#22252A"
  interaction-blue: "#047BF4"
  interaction-blue-light: "#255DA5"
  on-interactive-dark: "#101316"
  on-interactive-light: "#F4F7FD"
  brand-orange: "#EC4E02"
  success: "#029219"
  warning: "#DFA700"
  danger: "#E52222"
  info: "#1DB8CE"
  accent: "#FEAF7B"
  accent-light: "#7B3D05"
typography:
  display:
    fontFamily: "\"IBM Plex Sans\", ui-sans-serif, system-ui, -apple-system, sans-serif"
    fontSize: "40px"
    fontWeight: 600
    lineHeight: 1.2
    letterSpacing: "-0.02em"
  heading:
    fontFamily: "\"IBM Plex Sans\", ui-sans-serif, system-ui, -apple-system, sans-serif"
    fontSize: "24px"
    fontWeight: 600
    lineHeight: 1.35
    letterSpacing: "normal"
  body:
    fontFamily: "\"IBM Plex Sans\", ui-sans-serif, system-ui, -apple-system, sans-serif"
    fontSize: "15px"
    fontWeight: 400
    lineHeight: 1.5
    letterSpacing: "normal"
  label:
    fontFamily: "\"IBM Plex Sans\", ui-sans-serif, system-ui, -apple-system, sans-serif"
    fontSize: "11px"
    fontWeight: 500
    lineHeight: 1.35
    letterSpacing: "0.04em"
  machine-text:
    fontFamily: "\"IBM Plex Mono\", ui-monospace, SFMono-Regular, monospace"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.5
    letterSpacing: "normal"
rounded:
  sm: "4px"
  md: "6px"
  lg: "8px"
  xl: "12px"
  2xl: "16px"
  full: "9999px"
spacing:
  0: "0px"
  1: "2px"
  2: "4px"
  3: "6px"
  4: "8px"
  5: "12px"
  6: "16px"
  7: "20px"
  8: "24px"
  9: "32px"
  10: "40px"
  11: "48px"
components:
  button-primary:
    backgroundColor: "{colors.interaction-blue}"
    textColor: "{colors.on-interactive-dark}"
    typography: "{typography.label}"
    rounded: "{rounded.md}"
    padding: "6px 16px"
    height: "32px"
  button-primary-hover:
    backgroundColor: "{colors.interaction-blue}"
    textColor: "{colors.on-interactive-dark}"
  button-ghost:
    backgroundColor: "{colors.raised-dark}"
    textColor: "{colors.ink-dark}"
    rounded: "{rounded.md}"
    padding: "6px 16px"
    height: "32px"
  field:
    backgroundColor: "{colors.shell-dark}"
    textColor: "{colors.ink-dark}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: "0 12px"
    height: "32px"
  status-badge:
    backgroundColor: "{colors.shell-dark}"
    textColor: "{colors.ink-dark}"
    typography: "{typography.label}"
    rounded: "{rounded.full}"
    padding: "2px 8px"
---
# Design: MangoDisk

## Overview

MangoDisk is a Tauri 2 desktop disk analyzer and cleanup application. Rust owns file-system access, scan orchestration, cleanup execution, safety boundaries, and history; the interface helps a person understand storage and make reviewed choices. Its users locate large, duplicate, rebuildable, or application-associated data and understand the reclaimable impact before any mutation. `MangoDisk` is the current repository identity; the ByteDesk rename is not approved and this profile invents no replacement. Register: product. Direction lives in `PRODUCT.md`.

The creative north star is **The Explorable Landscape**: storage as terrain inside a restrained shell, where depth communicates directory hierarchy and selection consequence and never makes destructive cleanup feel playful or automatic. The composition motif is concentric capacity rings resolving into a navigable treemap. Personality: compact density in analysis with calm review and confirmation zones, measured motion, balanced richness, and a signature icon of a disk platter with a readable scanning eye and layered storage rings. Read after the shared foundation; this file names only what MangoDisk adds or changes.

## Colors

The accent is `--bd-accent` resolved through `data-bd-product="mangodisk"` (`product.mangodisk`). It marks the product in its mark and, as an edge-light, the selected level of the storage hierarchy. It never carries a destructive action, a warning, or a treemap fill; interaction blue keeps focus and selection, and semantic colour keeps risk.

Status vocabulary: success, warning, danger, and info always carry a word. The data vocabulary is exact and textual: estimated, selected, protected, rebuildable, personal, deleted, failed, and reclaimed are never conveyed by colour alone. Treemap cells take their tone from the family ground ramp (`theme.*.inset` through `theme.*.raised`) so hierarchy reads as depth, with semantic colour reserved for risk and protection. Richness defaults to `balanced` and the user may change it; all governed levels are supported. Dark and light ship as one interface.

## Typography

IBM Plex Sans for every interface surface. Sizes, paths, rule identifiers, and history entries are Sans at `type.body-sm` with `fontWeight.medium`; sizes and counts use tabular figures so columns align in the treemap table, the large-files list, and history. Paths truncate in the middle so the filename survives. No surface uses monospace: MangoDisk has no terminal, log viewer, or code content.

## Elevation

The capacity overview, treemap or list, and review zones sit on the shell plane. Optical layers map the storage hierarchy: each nested level steps one ground token deeper, and the selected level carries the edge-light. Base depth stays restrained. Scan scope, selection impact, preflight, confirmation, and partial-failure detail earn the raised and overlay levels because they are decisions. Glass is the shell only.

Density is compact in analysis views, declared here as the inspection mode; review and confirmation zones return to the family breathing-room floor so a destructive decision is never made in a cramped surface.

## Components

- **Capacity overview**: concentric rings with textual totals. States: empty disk, scanning, complete, unavailable volume.
- **Treemap and list**: synchronized views of one hierarchy with keyboard navigation. States: loading, partial, cancelled, permission denied.
- **Scan scope and progress**: read-only scanning with a visible cancel. States: queued, running, cancelled, complete.
- **Large files, duplicates, applications, protected paths**: review lists with exact size, risk, and protection words.
- **Cleanup rules**: rule identifier, scope, and estimated impact before enablement.
- **Selection impact and preflight**: reclaimable estimate, protection checks, and every consequence before confirmation.
- **Confirmation, execution, verification**: explicit confirmation, live execution, verified result, partial failure listed item by item.
- **History**: every mutation with rule, path, size, outcome, and time.

Motion is measured scan and reveal transitions; deletion and cleanup never use celebratory motion before verification. Stories and HTML mockups gate adoption for every surface above in both themes, all richness levels, responsive layouts, keyboard and focus, reduced motion, and the empty-disk, permission-denied, cancelled-scan, unavailable-volume, loading, error, offline, and every destructive state. Tauri adoption waits for explicit browser mockup approval.

## Do's and Don'ts

### Do

- Pair every treemap with a synchronized hierarchical table and full keyboard navigation.
- State sizes, risks, and protection in text.
- Return destructive focus predictably: after confirmation, execution, or cancellation, focus lands on the item or summary that changed.
- Keep scanning read-only, and require explicit confirmation, protection checks, and verifiable history for deletion, cleanup, uninstall, and startup changes.

### Don't

- No playful or automatic destructive flow; no celebration before verification.
- No trash can as the primary identity.
- No colour-only risk, protection, or reclaimed state.
- Exceptions to the foundation: none beyond the declared compact analysis mode.
- Generated art: nothing playful; scale reads large and the material solid; one plateau carries the edge-light, the selected level.
