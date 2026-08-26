# ByteDesk canonical tokens

This directory is the shared **value layer** of the ByteDesk family: shipping dark and
light themes, Black Glass + Optical Layering material values, governed dark richness,
product identity, cross-platform density/layout values, operational visualization
colors, and generated runtime adapters.

## Published forms

| File | Form | Consumer |
|---|---|---|
| `bytedesk.tokens.json` | DTCG-style canonical JSON | source of truth, tooling, native adapters |
| `css/bytedesk.css` | CSS custom properties (`--bd-*`) | browser, WebView, embedded HTML |
| `tailwind/theme.css` | Tailwind v4 `@theme` mapping | Next.js and Vite/React |
| `platforms/typescript/bytedesk-tokens.ts` | generated TypeScript map | canvas/chart tooling and non-CSS TypeScript |
| `platforms/rust/bytedesk_tokens.rs` | generated Rust constants | Tauri/native Rust adapters |
| `platforms/go/bytedesk_tokens.go` | generated Go constants | Go UI/tooling adapters |

`bytedesk.tokens.json` is authoritative. CSS values and derived aliases remain reviewed
human-readable adapters; TypeScript, Rust, and Go files are generated and checked.

Generate and verify:

```bash
node scripts/generate-platform-tokens.mjs
node scripts/generate-platform-tokens.mjs --check
node scripts/validate.mjs
```

## Family contract

### Ground, material, and surface

Every product surface starts from `color.bg.base` / `--bd-bg-base`. Dark is the default
and light is an equal shipping counterpart. Subtle, surface, elevated, and overlay form
the stable semantic ladder. Theme selectors remap those names; components never carry
separate dark/light geometry.

Black Glass + Optical Layering concentrates depth at the canvas/shell boundary and at
real hierarchy changes. A shell uses `--bd-shadow-shell`, controlled material blur, and
a fine top-light. Selected/expanded surfaces and overlays may use the raised level.
Ordinary rows and fields remain on their parent plane. This is structural glass, not a
frosted card around every value.

### Brand and interaction

- Brand orange identifies ByteDesk and conversion/commerce moments.
- Gateway blue is the shared interaction and focus family.
- Product accents identify products and providers; they never replace semantic status.
- The Gateway blue ramp and desk tints belong to operator-console chrome; the terminal
  and remote-surface stage stays on the base ground.

### Semantic status

Success, warning, danger, and info include foreground, background, and line roles.
Status is always icon/dot **plus a word**. Color is not the only carrier.

### Typography

IBM Plex Sans is the chrome family and IBM Plex Mono is for terminal output, commands,
paths, IDs, IPs, logs, and aligned machine values.

Web marketing type may use the fluid `clamp()` values in CSS. Native clients use the
static JSON values. Gateway console chrome uses `type.console` and `type.console-sm`.

### Density and layout

The fixed spacing ladder is:

```text
0, 2, 4, 6, 8, 12, 16, 20, 24, 32, 40, 48
```

Shared sizes define compact/default/touch controls and rows, pointer/touch targets,
icons, shell regions, panels, terminal tabs, and charts. Native values are logical
density-independent units.

Breakpoints are based on available content width:

- compact: up to 719;
- standard: 720–1199;
- wide: 1200–1599;
- ultrawide: 1600+.

Native clients apply those values to content width, not physical monitor pixels.

### Visualization

`color.chart.series-1` through `series-8` provide one ordered categorical palette for
browser and native renderers. Grid, axis, plot, and selection colors are also canonical.
Series always combine color with labels, markers, line style, direct values, or a table.

### Motion

Durations are 150, 250, and 400 ms with `ease-out-expo`. Consumers disable
non-essential motion when the browser or operating system requests reduced motion.
Terminal and remote-surface layout never animates for decoration.

## Product accents

| Product | Accent | Role |
|---|---|---|
| Platform | brand orange | ByteDesk suite |
| Gateway | Gateway blue | operator console |
| Vault | amber | identity/keys |
| Store | green | commerce identity |
| Workforce | violet | workforce identity |
| Agent Browser | cyan | browser identity |
| Agent Memory | pink | memory identity |
| Capture | bright blue | capture identity |
| Toolbox | electric cobalt | command-shelf identity |

Where a product accent equals a semantic color, identity and status remain separate:
status still requires a word and semantic component treatment.

## Consumption by runtime

### Browser / WebView

Import the canonical CSS and optional Tailwind adapter from the pinned design-system
mount:

```css
@import "../../../.context/design-system/tokens/css/bytedesk.css";
@import "../../../.context/design-system/tokens/tailwind/theme.css";
```

The consumer token root aliases local names to `--bd-*`. It does not copy literals.
Canvas or TypeScript-only code may import the generated TypeScript adapter.

### Rust desktop/native

Use `platforms/rust/bytedesk_tokens.rs` as generated source or copy it through a
deterministic build step. Map the raw canonical values once into the renderer's typed
theme:

```text
color.bg.base          -> application/window ground
color.text.primary     -> primary text
product.gateway        -> active product accent
size.control.compact   -> dense pointer control
size.control.touch     -> coarse-pointer control
color.chart.series-*   -> visualization palette
```

The typed mapping belongs in a client adapter module, not in individual components.
Expose `TOKEN_SOURCE_SHA256` in diagnostics.

### Go desktop/native or embedded UI

Use `platforms/go/bytedesk_tokens.go` or inline the CSS at build time for embedded HTML.
A Go renderer maps the generated raw values into its toolkit theme once. Do not maintain
a separate Go palette.

### TypeScript tooling and charts

`platforms/typescript/bytedesk-tokens.ts` preserves strings, numbers, and arrays from
the canonical JSON. Use it for chart/canvas values that cannot reliably consume CSS
custom properties, for token diagnostics, and for visual regression metadata.

## Consumer-local adapters

The inheritance order remains:

1. shared canonical values here;
2. product profile decisions under `profiles/<product>/`;
3. one consumer-local adapter for toolkit names and explicit implementation exceptions.

A local adapter may translate `color.bg.base` to `Visuals.panel_fill`, a Slint global, a
Tauri theme object, or a CSS alias. It may not introduce product values that belong in
the canonical or profile layer.

## Accessibility

- Target WCAG 2.2 AA.
- Respect text scaling, reduced motion, and contrast preferences.
- Touch targets use the touch size even when the visual control remains compact.
- Charts provide non-color distinction and a table/value alternative.
- Focus uses the canonical focus color and stroke.
- `text.on-brand` remains AA large-text only on brand orange; avoid it for small text.
- Danger as plain text on the base ground is not sufficient by color alone; pair it with
  label/icon and component background/line treatment.

## Themeability

Dark and light are both approved and use the same semantic names. Web consumers set
`data-bd-theme="dark|light"`; native consumers select the equivalent typed theme. The
system preference may choose the initial mode, but an explicit user choice persists.
Do not create per-product or per-platform light palettes.

Exact parity is mandatory: theme changes may alter color, translucency, shadow,
highlight, and glow values only. Content, geometry, hierarchy, controls, and component
states remain identical.

Dark consumers may expose `data-bd-richness="soft|balanced|rich"`. The setting adjusts
only dark canvas/shell depth and ambient strength. `balanced` is the default; light mode
ignores richness. Richness never changes contrast requirements, semantic state, layout,
or typography.

Product scoping on web uses `data-bd-product`; native clients select the equivalent
product accent in their theme object. Gateway desk tints use `data-bd-desk` or an
equivalent native desk role and apply only to chrome.

## Generated adapters

See [`platforms/README.md`](platforms/README.md). Generated files carry the source JSON
SHA-256 and are verified by `scripts/validate.mjs`.

## Product personality

`product.*` accents are only one part of personality. Each product profile also declares
its icon metaphor, density, surface calibration, motion temperament, voice, and domain
composition motif. The family material, interaction blue, restrained orange, semantic
status, theme geometry, and accessibility remain shared.

## What this layer is not

- not a framework-specific component library;
- not application runtime code;
- not a place for product-local literals;
- not a substitute for the Gateway component and visualization contracts;
- not a source of third-party surface themes beyond best-effort configuration.
