# Migrating from Moltis

Antre is a fork of [Moltis](https://github.com/moltis-org/moltis). An
existing Moltis install keeps working: Antre reads the old names and moves
what it owns on first start.

## What happens on first start

| Moltis | Antre | How |
|---|---|---|
| `~/.moltis/` | `~/.antre/` | moved, symlink left at `~/.moltis` |
| `~/.config/moltis/` | `~/.config/antre/` | moved, symlink left at `~/.config/moltis` |
| `moltis.db` (+ `-wal`, `-shm`) | `antre.db` | renamed inside the data directory |
| `docs/moltis/GUIDELINES.md` | `docs/antre/GUIDELINES.md` | renamed inside the data directory |
| `moltis.toml` / `.yaml` / `.yml` / `.json` | `antre.*` | read as is, with a warning |
| `MOLTIS_*` environment variables | `ANTRE_*` | accepted, with a warning |

Every step is logged at startup. Nothing is deleted or overwritten:

- a directory moves only when it is the default one in use (no
  `--data-dir`/`--config-dir` or `ANTRE_DATA_DIR`/`ANTRE_CONFIG_DIR`
  pointing elsewhere) and the new directory does not exist yet;
- the move is a rename, never a copy: if it fails (for instance the new path
  is on another filesystem), Antre keeps using the old directory;
- a file is renamed only when the new name is free;
- each move or rename appends a line to `.migrated-from-moltis` in the
  directory concerned.

When both `antre.toml` and `moltis.toml` exist, `antre.toml` wins. When both
`ANTRE_X` and `MOLTIS_X` are set, `ANTRE_X` wins.

## What to update yourself

- **Environment and service files**: rename `MOLTIS_*` to `ANTRE_*`, and the
  `moltis` binary to `antre`. The Nix package still ships a `moltis`
  command that prints a warning and runs `antre`.
- **Config file**: rename `moltis.toml` to `antre.toml`. If a tool writes
  the file on every start (a NixOS `preStart`, a container entrypoint),
  change it there, then delete the old file: otherwise the stale copy keeps
  being written next to the new one.
- **Containers**: the image now runs as user `antre` with
  `HOME=/home/antre`. Mount your volumes at `/home/antre/.antre` and
  `/home/antre/.config/antre` instead of the `moltis` paths.
- **Metrics and log filters**: metric names and tracing targets now start
  with `antre`, so dashboards, alerts and `RUST_LOG` filters naming
  `moltis` need the new prefix.
- **Web UI**: browser-side preferences (theme, layout) were stored under
  `moltis` keys and start from defaults once. You sign in again once,
  since the session cookie was renamed.
- **Background services**: `antre service install` writes new unit files;
  remove the old `moltis` ones (`moltis.service`, the launchd plist).

## Rolling back

Stop Antre, then undo what `.migrated-from-moltis` lists: remove the
`~/.moltis` symlink, move `~/.antre` back to `~/.moltis` (same for the
config directory), and rename `antre.db*` back to `moltis.db*`.

## Not renamed yet

Bundle identifiers (`org.moltis.*`), the launchd labels, the desktop app's
keychain URL scheme and the `moltis.org` hosts keep their Moltis names for
now. The repository, release, container image (`ghcr.io/antre-ai/antre`)
and Homebrew tap (`antre-ai/tap/antre`) links already point at the
`antre-ai` organization, which has nothing published yet.
