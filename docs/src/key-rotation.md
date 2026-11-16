# Key rotation

Your signing key is the root of trust for your game. Rotate it when you want
to retire it (new machine, suspected exposure, policy) while you still have
it. Rotation produces a **key-rotation statement**: a small JSON object saying
"`old_key` hands over `slug` to `new_key`", signed with the old key (see
SPEC §4.4).

## Procedure

1. Back up the current key: `gamedist key export <old-name>`.
2. Make sure you are logged in to every server in `gamedist.toml`.
3. Rotate:

   ```sh
   gamedist key rotate cool-game-2027
   ```

   This generates the key `cool-game-2027` (or reuses it if a key with that
   name exists), signs the rotation statement with the old key, and on every
   server in `gamedist.toml`:
   registers the new public key with your account and submits the statement
   (`POST /api/v0/games/<slug>/rotations`). Finally it switches `key` in
   `gamedist.toml` to the new name.
4. **Publish a new release** signed with the new key and promote it:

   ```sh
   gamedist push --version 1.3.0 --build … --channel stable
   ```

   The server clears all channels when it accepts a rotation, because they
   pointed at releases signed by the old key. Until you promote a new-key
   release, players see no current version.
5. Back up the new key. Keep the old one offline until every server you care
   about has accepted the rotation; then destroy it if it was exposed.

The game manifest now lists the new `developer_pubkey` and carries the
statement in `rotations`. Because the Game ID is derived from the key, the
Game ID changes too; the rotation statement is what links the old and new IDs.
Mirrors that are not in your `gamedist.toml` need the statement as well — add
them to `servers` before rotating, or mirror again afterwards.

## What the launcher does

A launcher that has the old key pinned and sees a release signed by the new
key looks for a valid chain of rotation statements (in the game manifest)
from the pinned key to the new key. Each link must be signed by its
`old_key` and name the same slug. If the chain is valid, the launcher moves
the pin to the new key and carries the install over without asking. If there
is no valid chain, it shows the hard key-change warning described in
[Launcher](launcher.md#the-key-change-warning).

The chain check is `rotation_chain_links` in `gamedist-core`; the launcher
applies it whenever it contacts a source (`add`, `install`, `update`,
`check`). The end-to-end tests cover this: after `gamedist key rotate` and a
new-key release, a launcher with the old pin updates to the new release
without a prompt, as a delta update, and its pin and Game ID move to the new
key.

## If the old key is lost

A lost key cannot sign a rotation statement, so there is no cryptographic
link to a new key. Consequences:

- The server will not accept releases signed by another key for the existing
  game, and neither the CLI nor the admin pages can rebind a slug to an
  unrelated key (a takedown does not free the slug). In practice you publish
  under the new key as a new game (new Game ID): a new slug, or the same slug
  on a server where it is still free.
- Players who installed the old game get no automatic update. If they are
  pointed at a release with the same slug and a new key, they get the hard
  key-change warning and must decide to trust the new key themselves.
- Announce the new key through channels players already trust (your website,
  social accounts) so they can confirm it.

This is the price of servers not being trusted: nobody, including the server
operator, can vouch for a new key on your behalf. Keep an offline backup.

## If the old key is compromised

Rotate immediately while you still control the account. Whoever holds the old
key can also sign a rotation statement to a key of their own, and can sign
releases that launchers with the old pin accept. Ask server operators to take
down releases you did not publish, and announce the new key publicly.
