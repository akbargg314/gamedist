# Mirroring and federation

GameDist has no central registry. Servers are independent; a game can be
hosted on as many of them as its developer (or anyone) likes, and launchers
treat all copies as the same game.

## Same Game ID everywhere

The Game ID is derived only from the developer's public key and the slug:

```
game_id = base32(SHA-256(pubkey_bytes ‖ slug))[0..26]
```

So `https://a.example/games/cool-game` and `https://b.example/games/cool-game`
published with the same key are the same game. A launcher that installed it
from one host keeps the same pinned key, Game ID and content store when you
add the other host, and can fetch chunks from either. A different developer
key under the same slug is a different game with a different Game ID.

In the launcher, adding the second URL adds that host as an extra source of
the existing library entry; installs and updates try the sources in order,
so the game stays installable when one host goes away. Chunk IDs do not
depend on the host, so with `keep_chunks` on, chunks downloaded from one host
are reused for the other. The end-to-end tests check exactly this: the same
game added from two servers shares Game ID, pinned key and content store, and
reinstalls through the mirror without downloading anything after the origin
is shut down.

## `gamedist mirror`

```sh
gamedist login https://b.example
gamedist mirror https://b.example                 # all versions
gamedist mirror https://b.example --version 1.2.0 # one version
```

Run in the project directory. The source is the first server in
`gamedist.toml` that is not the target. The command:

1. reads the game manifest from the source (no login needed there);
2. registers the game's public key with your account on the target and
   creates the game if it does not exist (it refuses if the target already has
   a game with that slug under a different key);
3. copies store-page assets;
4. for each version (oldest first), copies the chunks the target is missing
   and publishes the **original signed manifest** byte for byte;
5. sets the target's channels to match the source for the mirrored versions;
6. adds the target to `servers` in `gamedist.toml`.

No signing key is needed: mirroring re-publishes what the developer already
signed. Afterwards `gamedist push` publishes to both servers.

Registering a public key does not prove possession of the private key, so
anyone with an account on the target can mirror a game. This is intended: the
target checks every release signature against the game's key, and launchers
check it again, so a mirror can host a game but cannot alter it.

## Catalogs

A server with `catalog = true` (the default) exposes:

- `GET /api/v0/games?q=&page=` — paged search used by launcher Discover;
- `GET /api/v0/export.jsonl` — every public game manifest, one JSON object per
  line, for aggregators and indexers.

Taken-down games are excluded. An aggregator can merge several exports and
group entries by `game_id`; each line carries `developer_pubkey`, `channels`
and the `releases` URL, so clients can verify everything against the signed
release manifests. Importing another server's catalog into a server is not
implemented.

## Launcher catalogs

Players add catalog servers in the launcher settings; Discover then shows the
union of their catalogs. See [Launcher](launcher.md).
