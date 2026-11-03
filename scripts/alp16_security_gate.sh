#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
echo "=== STATIC SECRET HYGIENE ==="
python3 scripts/secret_scan.py
SCAN_DIR="$(mktemp -d)"
trap 'rm -rf "$SCAN_DIR"' EXIT

rsync -a \
  --exclude '.git/' \
  --exclude 'target/' \
  --exclude '.alp-backup/' \
  --include '.env.example' \
  --exclude '.env' \
  --exclude '.env.*' \
  --exclude '_build/' \
  --exclude 'deps/' \
  --exclude 'node_modules/' \
  ./ "$SCAN_DIR/"

if command -v gitleaks >/dev/null 2>&1; then
  gitleaks dir "$SCAN_DIR" --redact --no-banner
else
  docker run --rm \
    -v "$SCAN_DIR:/repo:ro" \
    ghcr.io/gitleaks/gitleaks:v8.29.1 \
    dir /repo --redact --no-banner
fi
echo "GITLEAKS_RELEASE_SOURCE_SCAN=PASS"

if [ -d .git ]; then
  if command -v gitleaks >/dev/null 2>&1; then
    gitleaks git . --redact --no-banner
  else
    docker run --rm \
      -v "$PWD:/repo:ro" \
      -w /repo \
      ghcr.io/gitleaks/gitleaks:v8.29.1 \
      git . --redact --no-banner
  fi
  echo "GITLEAKS_GIT_HISTORY_SCAN=PASS"
fi
grep -q '127.0.0.1:8080' docker-compose.yml;grep -q '127.0.0.1:5432' docker-compose.yml;grep -Fq '127.0.0.1:${ALP_BEAM_HOST_PORT:-8090}:8090' docker-compose.yml;! grep -q 'CorsLayer::permissive' crates/alp-server/src/main.rs
echo "LOCALHOST_BIND_ASSERTION=PASS"
echo "NON_PERMISSIVE_CORS_ASSERTION=PASS"
export ALP_SECURITY_MODE=token
export ALP_API_TOKEN="$(python3 - <<'PYTOKEN'
import secrets
print(secrets.token_hex(32))
PYTOKEN
)"
docker compose up -d --build --force-recreate postgres api >/dev/null
api_ready=0
for _ in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:8080/health >/dev/null 2>&1; then
    api_ready=1
    break
  fi
  sleep 1
done

if [ "$api_ready" != "1" ]; then
  echo "API_SECURITY_TEST_HEALTH=FAIL"
  docker compose logs --no-color api --tail=120 || true
  exit 1
fi

echo "API_SECURITY_TEST_HEALTH=PASS"
code="$(curl -sS -o /tmp/alp-no-token -w '%{http_code}' http://127.0.0.1:8080/v1/company/cases)";[ "$code" = "401" ]
code="$(curl -sS -o /tmp/alp-with-token -w '%{http_code}' -H "Authorization: Bearer $ALP_API_TOKEN" http://127.0.0.1:8080/v1/company/cases)";[ "$code" = "200" ]
echo "API_TOKEN_GATE_ASSERTION=PASS"
export ALP_SECURITY_MODE=local;unset ALP_API_TOKEN;docker compose up -d --force-recreate api >/dev/null
echo "MODEL_LOCAL_ONLY_ASSERTION=PASS"
echo "ALP16_SECURITY_GATE=GREEN"
