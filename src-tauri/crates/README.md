# Rust crates

This directory contains reusable Rust capabilities:

- `mangodisk-core` owns product domains, use cases, rules, indexing, cleanup,
  history, and reporting.
- `mangodisk-platform` implements macOS and Windows contracts for volumes,
  paths, links, system exclusions, application inventory, and permanent deletion.
- `mangodisk-cli` is a sibling adapter over Core use cases.
- `mangodisk-mcp` is a sibling adapter that exposes Core use cases as Model
  Context Protocol tools over stdio or bearer-authenticated loopback HTTP.
- `mangodisk-acp` bridges the desktop app to locally authenticated provider
  CLIs over the Agent Client Protocol for the in-app chat.

The Tauri crate only assembles the application, converts command arguments,
and forwards progress events. It does not own platform policy or scanning
behavior.
