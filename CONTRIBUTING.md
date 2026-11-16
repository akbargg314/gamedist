# Contributing to GameDist

Thanks for helping. GameDist is MIT-licensed and contributions are accepted
under the same license.

## Sign-off (DCO), no CLA

There is no Contributor License Agreement. Instead, every commit must carry a
`Signed-off-by` line certifying the [Developer Certificate of
Origin](https://developercertificate.org/):

```sh
git commit -s -m "server: reject oversized chunk uploads"
```

The sign-off must use your real name (or a consistent, identifiable pseudonym)
and an email address that reaches you. To fix a commit you forgot to sign:
`git commit --amend -s` (or `git rebase --signoff main` for a series).

## Coding rules

- Rust 2021. Code must pass `cargo fmt --all --check` and
  `cargo clippy --workspace --all-targets -- -D warnings`.
- No `unsafe` (the workspace denies `unsafe_code`).
- Every public API in `gamedist-core` has unit tests. Every server endpoint has
  an integration test (see `crates/server/tests/`, which run the router against
  a temporary SQLite database).
- Never trust manifest paths: reject `..`, absolute paths, drive letters and
  symlinks on assembly. Use the helpers in `crates/core/src/paths.rs`.
- Every network download is verified (BLAKE3) before it reaches its final
  location; write to a temp file and rename atomically.
- Never log secrets (API tokens, passwords, private keys). The CLI stores
  tokens in the OS keyring when available.
- Keep `gamedist-core` free of UI and server dependencies.
- Dependencies must be MIT / Apache-2.0 / BSD / ISC / Zlib / Unicode (or
  another entry in `deny.toml`). No GPL, AGPL or LGPL. `cargo deny check
  licenses bans sources` enforces this in CI.
- Protocol changes start in `protocol/SPEC.md` and the JSON Schemas, not in
  code. Record design decisions in `docs/decisions/`.
- Update `docs/` and `CHANGELOG.md` with user-visible changes.

## Running the checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check licenses bans sources   # cargo install cargo-deny
```

Run a single crate's tests with `cargo test -p gamedist-server` (or
`gamedist-core`, `gamedist-cli`, `gamedist-launcher`). The end-to-end tests,
which run the real server, CLI, launcher and sample game together (including
a BitTorrent swarm between launchers on localhost), are
`cargo test -p gamedist-e2e`. Server log output is controlled by `RUST_LOG`,
e.g. `RUST_LOG=gamedist_server=debug`.

The launcher GUI lives in `apps/launcher/`. Its frontend is checked with
`npm run check` and `npm run build` (or `deno task check` / `deno task
build`). Its Rust shell, `apps/launcher/src-tauri`, is a separate Cargo
workspace that needs the system webview (WebKitGTK on Linux) and is not part
of `cargo test --workspace`; see `apps/launcher/README.md`.

## Pull requests

- Keep PRs focused; describe what changed and why.
- CI (fmt, clippy, tests on Linux/Windows/macOS, cargo-deny, launcher
  frontend, sample-game builds) must pass.
- Security issues: please do not open a public issue; contact the maintainers
  privately first.
