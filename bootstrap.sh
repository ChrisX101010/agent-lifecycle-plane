#!/usr/bin/env bash
set -euo pipefail

echo "== Agent Lifecycle Plane v0.1 bootstrap =="

need() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "ERROR: $1 is required" >&2
        exit 1
    }
}

need docker
need curl
need python3

docker compose version >/dev/null

if [ ! -f .env ]; then
    cp .env.example .env
fi

python3 - <<'PYBOOT'
from pathlib import Path
import secrets

path = Path(".env")
lines = path.read_text().splitlines()
out = []
found = False

for line in lines:
    if line.startswith("ALP_POSTGRES_PASSWORD="):
        found = True
        value = line.split("=", 1)[1].strip()
        if not value or value.lower() in {"changeme", "change_me", "example", "alp_local_dev"}:
            line = "ALP_POSTGRES_PASSWORD=" + secrets.token_hex(32)
    out.append(line)

if not found:
    out.insert(0, "ALP_POSTGRES_PASSWORD=" + secrets.token_hex(32))

path.write_text("\n".join(out) + "\n")
path.chmod(0o600)
PYBOOT

echo "Local .env ready (mode 0600; secrets are not printed)."

docker compose up --build -d postgres api

echo "Waiting for ALP API..."

for _ in $(seq 1 40); do
    if curl -fsS http://127.0.0.1:8080/health >/dev/null; then
        echo "ALP_BOOTSTRAP=GREEN"
        exit 0
    fi
    sleep 2
done

echo "ALP_BOOTSTRAP=RED" >&2
docker compose ps
docker compose logs --tail=150 api
exit 1
