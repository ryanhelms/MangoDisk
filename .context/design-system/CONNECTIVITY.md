# Design-system to consumer connectivity

This document explains how a ByteDesk design decision becomes durable source, reaches a
consumer repository, and remains verifiable in CI. It is delivered with the managed
payload so humans and agents see the same contract from either side.

## Authority flow

```text
ByteDeskAI/design-system
  shared DESIGN.md + tokens + profile + approved assets/artifacts
        |
        | commit: immutable source SHA
        v
design-system.manifest.json
  checksummed capability and file graph
        |
        | scripts/publish-plugin.mjs
        v
bytedesk-marketplace/design-system
  provider manifests + MCP + skills + embedded payload
        |
        | design-system-sync --app <product>
        v
consumer/.context/design-system/
  shared foundation + tokens + selected profile + shared approved references/assets
        |
        | committed in consumer + CI design-system-sync --check
        v
consumer adapter and implementation
  local token aliases, toolkit mapping, Storybook/HTML, then approved runtime adoption
```

The source Git SHA is the design-system version. A marketplace installation is a
delivery mechanism, not an alternate authority. A consumer checkout never follows
`main` implicitly.

## Ownership by layer

### Canonical source repository

`ByteDeskAI/design-system` owns:

- family visual language and accessibility rules;
- canonical semantic tokens and platform adapters;
- shipping dark/light and governed richness contracts;
- one profile per product;
- approved brand/product assets and family reference artifacts;
- the manifest, sync runtime, MCP, and publication contract.

A consumer discovery or mockup can motivate a change, but the decision becomes shared
truth only after it lands here.

### Marketplace plugin

`bytedesk-marketplace/design-system` carries a checksummed, provider-neutral payload
generated from a clean committed source checkout. It provides discovery, skills, MCP,
and sync tooling. It must not contain hand-authored design forks.

The publisher stamps:

- the design-system source SHA;
- a manifest of every payload file, size, and SHA-256;
- provider-native metadata that points to the same capability.

Internal plugin versions remain commit-derived where the marketplace contract requires
that behavior. A stale local plugin cache is not proof that a consumer is current.

### Consumer repository

The consumer commits `.context/design-system/` as ordinary files because CI, Tauri,
Go embedding, Next builds, and TeamCity cannot read a developer's plugin cache. The
consumer receives:

- shared `DESIGN.md` and this connectivity contract;
- canonical tokens and the relevant runtime adapters;
- only its selected `profiles/<product>/` profile;
- cataloged identity assets and approved family references distributed by the payload.

Managed files are read-only in the consumer. Local implementation details live in the
consumer root `DESIGN.md`, token adapter, Storybook, HTML prototypes, and runtime source.
Do not patch `.context/design-system/` to make a local build pass.

### Project marketplace wiring

Every consumer commits `.claude/settings.json`. Register the marketplace by a path
relative to the consumer's main checkout and enable the internal plugin beside it:

```json
{
  "extraKnownMarketplaces": {
    "bytedesk": {
      "source": { "source": "directory", "path": "../bytedesk-marketplace" },
      "autoUpdate": true
    }
  },
  "enabledPlugins": { "design-system@bytedesk": true }
}
```

The literal relative path depends on repository layout. ByteDeskAI siblings normally
use `../bytedesk-marketplace`; a checkout beside the `ByteDeskAI/` directory uses
`../ByteDeskAI/bytedesk-marketplace`. Never commit an absolute workstation path.
Project trust may be required before Claude Code activates the declaration.

Do not ignore `.claude/settings.json`. Ignore only machine-local plugin caches,
worktrees, telemetry, and `settings.local.json`.

## Reading order in a consumer

1. `.context/design-system/DESIGN.md`
2. `.context/design-system/profiles/<product>/DESIGN.md`
3. adjacent `PRODUCT.md`, when present
4. consumer root `DESIGN.md` for toolkit mappings and explicit exceptions

An exception names the inherited rule it changes, explains why, and identifies whether
it is temporary. Copying upstream prose into a local file does not create an override.

## Authoring and release workflow

1. Author shared decisions at the narrowest correct layer.
2. Add or update tests, checksums, and provenance.
3. Generate platform token adapters.
4. Commit source files.
5. Record that commit in `catalog.json` for newly authored profiles/artifacts.
6. Generate and commit `design-system.manifest.json` from committed bytes.
7. Run repository tests, validation, release verification, and clean-copy checks.
8. Publish into the marketplace plugin from the clean commit.
9. Validate the plugin and commit its payload update in the marketplace repository.
10. Sync the consumer, inspect the diff, and commit the vendored context.
11. In CI, checkout the public `ByteDeskAI/bytedesk-marketplace` repository and run
    its `design-system-sync --app <product> --check` against the consumer. Also run
    the committed `.bytedesk/design-system-check.mjs` for cache-independent integrity.

Publication refuses dirty source because a payload must never contain bytes that its
stamped Git SHA cannot reproduce.

## Consumer sync lifecycle

```bash
node <plugin>/bin/bd-design init --app <product> --dry-run
node <plugin>/bin/bd-design init --app <product>

node <plugin>/scripts/design-system-sync.mjs --app <product> --dry-run
node <plugin>/scripts/design-system-sync.mjs --app <product>
node <plugin>/scripts/design-system-sync.mjs --app <product> --check
node <plugin>/scripts/design-system-sync.mjs --app <product> --doctor
```

`--dry-run` shows the exact add/change/delete plan. Apply is atomic. `--check` verifies
the source SHA, selected profile, file set, and checksums without needing the private
source repository. `--doctor` also explains missing consumer wiring.

The two CI checks answer different questions:

- plugin `design-system-sync --check`: is this consumer current with the published
  marketplace payload and selected profile?
- committed `design-system-check.mjs`: is the vendored tree internally complete and
  checksummed even when no plugin cache or source checkout exists?

## Mockup, Storybook, and upstream feedback

Consumer HTML mockups and Storybook stories are implementation evidence, not a second
design system. The loop is:

```text
canonical rule/profile
  -> image direction
  -> approved HTML + Storybook states
  -> explicit human approval
  -> native/runtime adoption
  -> newly discovered shared need proposed upstream
```

If implementation reveals a missing shared token, material rule, component state, or
identity asset, stop inventing local literals and land the decision upstream first.
Product-specific component composition remains in the product profile or consumer.

## Reference and asset delivery

Production identity files live under `assets/` and require catalog provenance and a
checksum. Approved family visual records live under `artifacts/` and are authoring-only
unless their catalog entry explicitly declares payload distribution. Distributed
artifact files are also checksummed and appear in CLI/MCP discovery.

Raster references guide material and visual-regression work. Written contracts and
tokens resolve ambiguous pixels. Never ship a full reference screenshot as application
UI, and never infer production behavior from mock data pictured in it.

## Drift and recovery

- A consumer `.source-sha` mismatch means the vendored context and installed plugin do
  not describe the same source revision. Refresh the plugin, then sync again.
- A checksum or managed-file mismatch means the consumer tree was edited or partially
  copied. Review local changes; restore through sync rather than manual repair.
- An installed plugin can be stale even when enabled. Verify the source SHA and perform
  a representative read-only MCP/CLI call in a fresh provider session.
- A profile missing from the payload is an upstream publication defect. Do not copy it
  from another consumer.
- Consumer-local adapters remain outside the managed directory and must be reviewed
  whenever token or profile contracts change.
