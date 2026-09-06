# TM-097 batch three — MangoDisk 2.2.1 adoption

Prepared on `feat/tm097-adoption-2.2.1`, baseline `a13376c`. Lead retains commits,
publication, review and task acceptance. No deployment or native release was performed.

## Implementation

1. Baseline: installed the original frozen lockfile, built the actual Vue frontend,
   and captured the startup cleanup screen in both implemented themes at 1440×1000
   and the supported native minimum, 1000×700. No invented compact/mobile surface.
2. Added exact `@bytedesk/design-tokens@2.2.1` and dev-only
   `@bytedesk/design-client@2.2.1`, plus the public scoped registry mapping.
   `main.css` imports the published token CSS before local skins. Existing 20px
   shell padding now consumes `--bd-space-7`; local skin colors, type, icons,
   component geometry and product identity remain unchanged. The shared product/theme
   stamps follow MangoDisk's existing explicit/system theme selection.
3. Added the scoped `mangodisk@2.2.1` pin. Official sync generated **45 payload files**
   plus lock/README, replacing the obsolete broad context. Anonymous delivery used
   empty user/global npm configs, a fresh cache and a minimal credential-free
   environment. No machine token was read or supplied.
4. Replaced the legacy marketplace checkout/payload workflow with installed
   `design-client sync --check`. Added `check:design-system` at the start of
   `pnpm check`, so the existing cross-platform workflow inherits the gate.
   Removed the unused legacy verifier; updated root design-authority references
   and `.envrc` to the scoped `foundation/` and `apps/mangodisk/` paths.
5. Required validation, visual comparison, runtime-consumption probe and safe UI
   interactions completed as recorded below. No remote workflow was started.

MangoDisk retains its name, original upstream package metadata, homepage and
**GPL-3.0-only** declaration. No rename, framework migration, cleanup rule,
filesystem behavior, Tauri permission or generated Shadcn component was changed.

## Results

| Check | Result |
| --- | --- |
| Original frontend build | Passed |
| Anonymous official sync | Passed; 45 files |
| Strict offline check | Passed; new network namespace, empty environment, actual `/usr/bin/node`, 20-second timeout |
| `pnpm check` | Passed, including identity/style/i18n/source guards, lint/format, frontend build, Rust fmt/clippy/workspace check |
| Frontend tests | 62 test files, 340 passed |
| Required `mangodisk-core` tests | 367 passed, 0 failed; 15 existing ignored diagnostics/integration tests |
| Before/after pixels | **0 changed pixels in all four pairs; byte-identical PNGs** |
| Geometry, local colors/type and padding | Identical in all four sampled cases |
| Shared-token consumption | Browser-only 20→24px token probe changed actual shell padding 20→24px and heading x260→264, then restored |
| Settings theme | Explicit Light synchronized both theme stamps while OS preference remained Dark |
| Navigation/menus | Sidebar expansion, settings navigation and scan-type menu open/Escape observed; no scan selected or started |

Rust compilation used `CARGO_BUILD_JOBS=2`, with complete checks and core tests
run serially. The 15 ignored cases are pre-existing opt-in real-volume,
platform-change-history and performance diagnostics. They were not enabled because
this task authorizes local fixtures, not personal filesystem workloads. Their
individual reasons remain in the sanitized core log. No skipped test is reported as passed.

## Visual and safety scope

Agent-browser MCP was reprobed: OSControl at 127.0.0.1:9224 refused the connection.
The fallback uses the installed **agent-browser CLI**, isolated session
`tm097-b3-mangodisk`, against the actual production bundle served locally on port4434.

`browser-fixture.js` supplies a deny-by-default in-memory Tauri IPC boundary before
the application boots. Its only volume is the explicitly fictional `Fixture volume`;
no native bridge, personal paths, files, installation identity or persisted user
settings are accessed. Settings edits remain in memory. Scan, cleanup, process
termination, native dialogs and unrecognized commands are refused. The optional
`filter_directory_paths` discovery command is deliberately refused and recorded;
it does not prevent the tested startup/navigation surface from rendering.

This is evidence for the **actual Vue UI with isolated backend fixtures**, not
native Windows/macOS WebView acceptance or disk-operation correctness. macOS and
Windows were unavailable; those platform checks remain unvalidated locally.
Browser interaction evidence uses English. Locale guards and the frontend tests
passed, but no claim of manual acceptance in every locale is made. No product text
or layout value changed in this adoption.

The zero delta is measured rather than assumed: `comparison.json` stores exact
ImageMagick absolute-error counts and PNG hashes. `before-metrics.json` and
`after-metrics.json` retain computed rectangles, colors, fonts, local tokens and
fixture command names. The imported package is genuinely used for spacing, while
MangoDisk's local skins continue to own its appearance. Shared profile sync alone
is not represented as a redesign or wholesale visual-system adoption.

## Reproduction and artifacts

```sh
pnpm install --frozen-lockfile
pnpm build
pnpm preview --host 127.0.0.1 --port 4434
# Against the original baseline build, then this candidate build:
python3 docs/evidence/TM-097-batch3/capture.py before
python3 docs/evidence/TM-097-batch3/capture.py after
python3 docs/evidence/TM-097-batch3/compare.py
python3 docs/evidence/TM-097-batch3/sync-anonymous.py
CARGO_BUILD_JOBS=2 pnpm check
CARGO_BUILD_JOBS=2 cargo test --manifest-path src-tauri/Cargo.toml -p mangodisk-core
```

The capture driver waits for the real page and fictional-volume label, fonts and
stable frames, using identical reduced-motion, viewport and theme setup for both
versions. It does not change source styles to manufacture parity. Test/build output (`before-build.txt`, `pnpm-check.txt`, `core-tests.txt`,
`sync-anonymous.txt`, `sync-offline-check.txt`) is sanitized to replace local checkout/home paths; no raw machine report is included.
All figures are preparation evidence, pending the lead's acceptance.
