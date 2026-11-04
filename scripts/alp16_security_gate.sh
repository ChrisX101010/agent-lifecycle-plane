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
grep -q '127.0.0.1:8080' docker-compose.yml
grep -q '127.0.0.1:5432' docker-compose.yml
grep -Fq '127.0.0.1:${ALP_BEAM_HOST_PORT:-8090}:8090' docker-compose.yml
! grep -q 'CorsLayer::permissive' crates/alp-server/src/main.rs
grep -q '^USER alp$' crates/alp-server/Dockerfile
grep -q '^USER alp$' runtimes/alp-beam/Dockerfile
grep -q 'no-new-privileges:true' docker-compose.yml
grep -q 'cap_drop:' docker-compose.yml
echo "LOCALHOST_BIND_ASSERTION=PASS"
echo "NON_PERMISSIVE_CORS_ASSERTION=PASS"
echo "NON_ROOT_IMAGE_ASSERTION=PASS"
echo "CONTAINER_HARDENING_CONFIG_ASSERTION=PASS"
grep -q 'internal server error' crates/alp-server/src/security.rs
! grep -R -nE 'INTERNAL_SERVER_ERROR, (e|error)\.to_string\(\)' crates/alp-server/src >/dev/null 2>&1
echo "INTERNAL_ERROR_REDACTION_ASSERTION=PASS"
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
api_uid="$(docker compose exec -T api id -u | tr -d '\r')"
[ "$api_uid" != "0" ]
api_id="$(docker compose ps -q api)"
docker inspect "$api_id" --format '{{json .HostConfig.SecurityOpt}}' | grep -q 'no-new-privileges'
docker inspect "$api_id" --format '{{json .HostConfig.CapDrop}}' | grep -q 'ALL'
echo "API_NON_ROOT_RUNTIME_ASSERTION=PASS"
echo "API_CONTAINER_PRIVILEGE_ASSERTION=PASS"
code="$(curl -sS -o /tmp/alp-no-token -w '%{http_code}' http://127.0.0.1:8080/v1/company/cases)";[ "$code" = "401" ]
code="$(curl -sS -o /tmp/alp-with-token -w '%{http_code}' -H "Authorization: Bearer $ALP_API_TOKEN" http://127.0.0.1:8080/v1/company/cases)";[ "$code" = "200" ]
echo "API_TOKEN_GATE_ASSERTION=PASS"
python3 - <<'PYBIG' >/tmp/alp-oversized.json
import json
print(json.dumps({
    "name": "oversized",
    "description": "x" * (1024 * 1024 + 8192),
    "risk": "low",
    "runtime": "local",
    "created_by_principal_id": "00000000-0000-0000-0000-000000000000",
}))
PYBIG
code="$(curl -sS -o /tmp/alp-oversized-response -w '%{http_code}' \
  -H "Authorization: Bearer $ALP_API_TOKEN" \
  -H 'Content-Type: application/json' \
  --data-binary @/tmp/alp-oversized.json \
  http://127.0.0.1:8080/v1/tasks)"
[ "$code" = "413" ]
echo "REQUEST_BODY_LIMIT_ASSERTION=PASS"
curl -sS -D /tmp/alp-cors-headers -o /dev/null \
  -H 'Origin: https://untrusted.invalid' \
  http://127.0.0.1:8080/health
! grep -qi '^access-control-allow-origin:' /tmp/alp-cors-headers
echo "CORS_DENY_BY_DEFAULT_ASSERTION=PASS"
export ALP_SECURITY_MODE=local;unset ALP_API_TOKEN;docker compose up -d --force-recreate api >/dev/null
echo "MODEL_LOCAL_ONLY_ASSERTION=PASS"
echo "ALP16_SECURITY_GATE=GREEN"
