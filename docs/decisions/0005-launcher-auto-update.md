# 0005. Launcher auto-update channel

Status: Accepted (not yet implemented)

## Context

PLAN §9.5: should the launcher update itself through the GameDist protocol
(published as a game on a GameDist server) or through GitHub Releases?

## Decision

Both, with the **protocol as the primary channel**: the launcher is published
like any game, signed with a dedicated project key, and updated through the
same verified install path. GitHub Releases is the fallback for bootstrapping
and for when no GameDist server is reachable.

## Consequences

- The launcher dogfoods its own update, verification and rollback logic.
- The launcher's signing key must be pinned in the binary and protected like
  any developer key; a rotation follows the normal rotation procedure.
- OS code signing of launcher binaries (planned for a later phase) is
  independent of this and still needed for installers.
- Nothing of this is implemented yet. The release workflow currently publishes
  headless launcher binaries to GitHub Releases only.
