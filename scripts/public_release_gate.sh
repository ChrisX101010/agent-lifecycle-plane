#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

need() { command -v "$1" >/dev/null 2>&1 || { echo "FAIL: missing $1" >&2; exit 1; }; }
need git
need python3
need docker

[ -d .git ] || { echo "FAIL: public release gate requires a Git worktree" >&2; exit 1; }

required=(README.md SECURITY.md LICENSE CHANGELOG.md .env.example Cargo.toml Cargo.lock docker-compose.yml)
for file in "${required[@]}"; do
    [ -f "$file" ] || { echo "FAIL: missing $file" >&2; exit 1; }
done
echo "PUBLIC_DOCS_ASSERTION=PASS"

if git ls-files | grep -E '(^|/)(\.alp-backup/|target/|_build/|deps/|node_modules/|.*\.pre-alp.*$|.*\.(pem|key|p12|pfx|jks)$)' >/dev/null; then
    echo "FORBIDDEN_TRACKED_FILE_ASSERTION=FAIL" >&2
    exit 1
fi

if git ls-files | grep -E '(^|/)\.env($|\.)' | grep -vE '(^|/)\.env\.example$' >/dev/null; then
    echo "FORBIDDEN_TRACKED_FILE_ASSERTION=FAIL" >&2
    exit 1
fi
echo "FORBIDDEN_TRACKED_FILE_ASSERTION=PASS"

STAGE_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGE_DIR"' EXIT
git checkout-index --all --prefix="$STAGE_DIR/"

if ! python3 - "$STAGE_DIR" <<'PYPATH'
from pathlib import Path
import re
import sys

root = Path(sys.argv[1])
patterns = [
    re.compile(r"/home/[^/\s]+"),
    re.compile(r"/mnt/c/Users/[^/\s]+", re.IGNORECASE),
    re.compile(r"C:\\Users\\[^\\\s]+", re.IGNORECASE),
]
skip = {Path("Cargo.lock"), Path("scripts/public_release_gate.sh")}

bad = []
for path in root.rglob("*"):
    if not path.is_file():
        continue
    rel = path.relative_to(root)
    if rel in skip:
        continue
    text = path.read_text(errors="ignore")
    for line_no, line in enumerate(text.splitlines(), 1):
        if any(rx.search(line) for rx in patterns):
            bad.append(f"{rel}:{line_no}:{line.strip()}")

if bad:
    print("\n".join(bad))
    sys.exit(1)
PYPATH
then
    echo "PERSONAL_PATH_ASSERTION=FAIL" >&2
    exit 1
fi
echo "PERSONAL_PATH_ASSERTION=PASS"

python3 scripts/secret_scan.py

if command -v gitleaks >/dev/null 2>&1; then
    gitleaks dir "$STAGE_DIR" --redact --no-banner
else
    docker run --rm -v "$STAGE_DIR:/repo:ro" ghcr.io/gitleaks/gitleaks:v8.29.1 \
        dir /repo --redact --no-banner
fi
echo "STAGED_GITLEAKS_ASSERTION=PASS"

git diff --cached --check
echo "GIT_INDEX_QUALITY_ASSERTION=PASS"

echo "STAGED_FILES=$(git ls-files | wc -l)"
echo "PUBLIC_RELEASE_GATE=GREEN"
