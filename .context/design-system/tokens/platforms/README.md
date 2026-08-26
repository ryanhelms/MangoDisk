# Generated platform token adapters

These files are generated from [`../bytedesk.tokens.json`](../bytedesk.tokens.json):

- `typescript/bytedesk-tokens.ts`
- `rust/bytedesk_tokens.rs`
- `go/bytedesk_tokens.go`

They let browser tooling, Rust desktop/native code, and Go native/tooling code consume
the exact same token names and values without hand-copying literals.

Generate:

```bash
node scripts/generate-platform-tokens.mjs
```

Verify:

```bash
node scripts/generate-platform-tokens.mjs --check
node scripts/validate.mjs
```

The generated Rust and Go adapters expose raw canonical values as strings. A consumer
maps them once into its renderer's typed color, dimension, duration, or easing types.
That mapping is the consumer-local adapter layer from the inheritance contract; visual
values do not belong in component source.

Generated files include the SHA-256 of `bytedesk.tokens.json`. Clients should expose that
checksum in diagnostics and their platform parity record so browser/native screenshots
can be tied to one design-system version.
