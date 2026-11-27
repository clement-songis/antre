#!/usr/bin/env python3
"""Mechanical Moltis -> Antre rebrand of the working tree.

Rewrites every tracked text file and renames every tracked path, preserving
case (``MOLTIS`` -> ``ANTRE``, ``Moltis`` -> ``Antre``, ``moltis`` -> ``antre``),
except for what must keep the old name (see ``EXCLUDED`` and ``RULES``).
Upstream GitHub links move to ``REBRAND_REPO`` (default ``antre-ai/antre``),
``*.moltis.org`` hosts to ``REBRAND_DOMAIN`` / ``REBRAND_DOCS_HOST``
(default: unchanged until a domain is chosen).

Re-applying on another branch (e.g. after a feature branch lands on a
pre-rebrand base):

    git switch <branch>              # clean tree, based on the pre-rebrand code
    python3 scripts/rebrand.py       # rewrite + rename
    # Shorter names shift alignment and line wrapping:
    just format && taplo fmt && biome check --write crates/web/ui/src/
    git checkout -- vendor docs/.mdbook-lint.toml   # taplo drift unrelated to the rename
    cargo check                      # Cargo.lock follows the renamed crates
    git add -A && git commit -m "refactor: rename Moltis to Antre (scripted)"

then cherry-pick or rebase the hand-written rebrand commits on top (they are
the commits after "refactor: rename Moltis to Antre (scripted)" on the
rebrand branch). The script is idempotent: a second run changes nothing.

``--check`` lists the files that still mention the old name outside the
kept spots and exits 1 if any would change, without writing anything.

Keeping the old name on purpose:
- a line containing ``rebrand:keep`` is left untouched;
- so is everything between ``rebrand:keep-start`` and ``rebrand:keep-end``;
- ``EXCLUDED`` paths are neither rewritten nor renamed;
- ``RULES`` decide what kept or mapped spots look like in rewritten files.
"""

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Paths (fnmatch-style globs on the repo-relative path) never touched.
EXCLUDED = [
    # History.
    "CHANGELOG.md",
    "docs/src/changelog.md",
    "plans/*",
    "prompts/*",
    ".beads/*",
    # Upstream copyright line stays as written.
    "LICENSE.md",
    # sqlx checksums every applied migration: editing one breaks existing DBs.
    "*/migrations/*",
    # Third-party code.
    "vendor/*",
    # Reads the legacy names on purpose.
    "crates/config/src/legacy_brand.rs",
    "crates/config/src/legacy_brand/*",
    "docs/src/legacy-migration.md",
    "scripts/rebrand.py",
]

# Where the project now lives. Override with environment variables.
REPO = os.environ.get("REBRAND_REPO", "antre-ai/antre")
# The domain is not chosen yet: by default links keep the current hosts.
DOMAIN = os.environ.get("REBRAND_DOMAIN", "moltis.org")
DOCS_HOST = os.environ.get("REBRAND_DOCS_HOST", f"docs.{DOMAIN}")

UPSTREAM_REPO = "moltis-org/moltis"
# Links to one upstream issue, PR, discussion, release or commit: they only
# exist upstream.
UPSTREAM_ONLY = re.compile(
    r"/(?:issues|pull|discussions)/\d+|/releases/tag/|/commit/|/blob/[0-9a-f]{7,}/"
)


def plain_replace(text: str) -> str:
    for old, new in REPLACEMENTS:
        text = text.replace(old, new)
    return text


def upstream_repo(match) -> str:
    rest = match.group("rest") or ""
    if UPSTREAM_ONLY.match(rest):
        return match.group(0)
    return REPO + plain_replace(rest)


def homebrew_tap(match) -> str:
    return REPO.split("/")[0] + "/" + plain_replace(match.group(0).split("/", 1)[1])


def host(match) -> str:
    sub = match.group("sub") or ""
    if sub == "docs.":
        return DOCS_HOST
    return sub + DOMAIN


def keep(match) -> str:
    return match.group(0)


# Checked in order at each position; the first match wins and its result is
# not rewritten further.
RULES = [
    (r"moltis-org/moltis#\d+", keep),
    (r"moltis-org/(?:homebrew-tap|tap(?:/moltis)?)(?![\w-])", homebrew_tap),
    (r"moltis-org/moltis(?![\w-])(?P<rest>(?:/[\w.%#-]+)*)", upstream_repo),
    # Other repositories of the upstream org (the sqlx fork, ProxmoxVED).
    (r"moltis-org(?:/[\w.%#-]+)*", keep),
    (r"(?P<sub>(?:[a-z0-9-]+\.)*)moltis\.org\b", host),
    # Bundle ids, App Group, launchd labels, desktop app id, container labels.
    (r"\borg\.moltis\b", lambda _m: "ai.antre"),
    # Key-derivation salt: changing it makes existing recovery keys useless.
    (r"moltis-vault-recovery-key-salt!!", keep),
    # Hashed into persisted sandbox ids.
    (r"moltis-sandbox-id-v2", keep),
]

REPLACEMENTS = [("MOLTIS", "ANTRE"), ("Moltis", "Antre"), ("moltis", "antre")]

BINARY_SUFFIXES = {
    ".png", ".jpg", ".jpeg", ".gif", ".ico", ".icns", ".webp", ".woff",
    ".woff2", ".ttf", ".otf", ".wasm", ".pdf", ".zip", ".gz", ".tgz",
    ".mp3", ".mp4", ".wav", ".ogg", ".onnx", ".bin", ".db", ".sqlite",
}

RULES_RE = re.compile("|".join(f"(?P<r{i}>{p})" for i, (p, _) in enumerate(RULES)))
BRAND_RE = re.compile("moltis", re.IGNORECASE)
PLACEHOLDER = "\0KEEP{}\0"


def is_excluded(rel: str) -> bool:
    from fnmatch import fnmatch

    return any(fnmatch(rel, pattern) for pattern in EXCLUDED)


def replace_brand(text: str) -> str:
    kept = []

    def stash(match):
        for i, (_, handler) in enumerate(RULES):
            if match.group(f"r{i}") is not None:
                kept.append(handler(_Sub(match, i)))
                break
        return PLACEHOLDER.format(len(kept) - 1)

    masked = plain_replace(RULES_RE.sub(stash, text))
    return re.sub("\0KEEP(\\d+)\0", lambda m: kept[int(m.group(1))], masked)


class _Sub:
    """One rule's view of a combined-regex match."""

    def __init__(self, match, index):
        self._match = match
        self._index = index

    def group(self, name=0):
        if name == 0:
            return self._match.group(f"r{self._index}")
        return self._match.group(name)


def rewrite_text(text: str) -> str:
    out = []
    keeping = False
    for line in text.splitlines(keepends=True):
        if "rebrand:keep-start" in line:
            keeping = True
        if keeping or "rebrand:keep" in line:
            out.append(line)
        else:
            out.append(replace_brand(line))
        if "rebrand:keep-end" in line:
            keeping = False
    return "".join(out)


def tracked_files():
    raw = subprocess.run(
        ["git", "ls-files", "-z"], cwd=ROOT, check=True, capture_output=True
    ).stdout
    return [p for p in raw.decode().split("\0") if p]


def read_text(path: Path):
    if path.suffix.lower() in BINARY_SUFFIXES or path.is_symlink():
        return None
    data = path.read_bytes()
    if b"\0" in data[:8192]:
        return None
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="report only, write nothing")
    args = parser.parse_args()

    rewritten, renamed = [], []
    for rel in tracked_files():
        if is_excluded(rel):
            continue
        path = ROOT / rel
        if not path.exists():
            continue
        text = read_text(path)
        if text is not None and BRAND_RE.search(text):
            new_text = rewrite_text(text)
            if new_text != text:
                rewritten.append(rel)
                if not args.check:
                    path.write_text(new_text, encoding="utf-8")
        new_rel = replace_brand(rel)
        if new_rel != rel:
            renamed.append((rel, new_rel))
            if not args.check:
                target = ROOT / new_rel
                if target.exists():
                    print(f"refusing to overwrite {new_rel}", file=sys.stderr)
                    return 2
                target.parent.mkdir(parents=True, exist_ok=True)
                os.rename(path, target)

    if not args.check:
        # Drop directories the renames left empty.
        for rel, _ in renamed:
            parent = (ROOT / rel).parent
            while parent != ROOT and parent.exists() and not any(parent.iterdir()):
                parent.rmdir()
                parent = parent.parent

    verb = "would rewrite" if args.check else "rewrote"
    print(f"{verb} {len(rewritten)} files, {'would rename' if args.check else 'renamed'} {len(renamed)} paths")
    if args.check:
        for rel in rewritten:
            print(f"  content: {rel}")
        for old, new in renamed:
            print(f"  path:    {old} -> {new}")
        return 1 if rewritten or renamed else 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
