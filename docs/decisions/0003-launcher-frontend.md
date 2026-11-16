# 0003. Launcher frontend: Tauri 2 + Svelte

Status: Accepted

## Context

PLAN §9.3: Svelte (default) or React for the Tauri launcher. The launcher's
backend must reuse `gamedist-core` directly, and the app should be small and
start quickly on Windows, Linux and macOS.

## Decision

The launcher GUI is a **Tauri 2** app with a **Svelte + TypeScript** frontend
in `apps/launcher/`. Its engine (library, install, update, headless CLI) lives
in the Rust crate `crates/launcher` so that it can be tested without a
webview.

## Consequences

- Small bundles using the system webview; no bundled Chromium.
- The Tauri shell needs system WebKitGTK on Linux, so it is built outside the
  main Cargo workspace (`apps/launcher/src-tauri` is its own workspace). CI
  builds and type-checks the frontend only; the shell is not built in CI and
  has not yet been compiled in the development environment.
- All logic that matters for security (verification, key pinning, path
  checks) is in Rust; the frontend only renders state and sends commands.
- Contributors need Node.js for the frontend in addition to Rust.
