#!/usr/bin/env bash
set -euo pipefail

REPO="${1:-$(pwd)}"
cd "$REPO"

if [ -z "${ALP_BEAM_HOST_PORT:-}" ] && [ -f .env ]; then
  ALP_BEAM_HOST_PORT="$(
    awk -F= '$1=="ALP_BEAM_HOST_PORT" {
      print substr($0, index($0, "=") + 1)
    }' .env | tail -n1
  )"
fi

ALP_BEAM_HOST_PORT="${ALP_BEAM_HOST_PORT:-8090}"
export ALP_BEAM_HOST_PORT

need() { command -v "$1" >/dev/null 2>&1 || { echo "FAIL: missing $1" >&2; exit 1; }; }
need docker
need curl
need python3

PSQL=(docker compose exec -T postgres psql -U alp -d alp -Atc)

wait_http() {
  local url="$1"
  for _ in $(seq 1 120); do
    if curl -fsS "$url" >/dev/null 2>&1; then return 0; fi
    sleep 0.5
  done
  echo "FAIL: health timeout: $url" >&2
  return 1
}

echo "=== BUILD + START ALP14 CONTROL PLANE ==="
docker compose up -d --build postgres api
wait_http http://127.0.0.1:8080/health
curl -fsS http://127.0.0.1:8080/health | python3 -m json.tool

OWNER="$("${PSQL[@]}" "SELECT p.id FROM principals p JOIN principal_capabilities pc ON pc.principal_id=p.id JOIN capabilities c ON c.id=pc.capability_id WHERE p.kind='user' AND p.status='active' AND c.name='task.approve' ORDER BY p.created_at LIMIT 1")"
PLANNER="$("${PSQL[@]}" "SELECT p.id FROM principals p JOIN agents a ON a.principal_id=p.id WHERE p.status='active' AND a.role='planner' LIMIT 1")"
RESEARCHER="$("${PSQL[@]}" "SELECT p.id FROM principals p JOIN agents a ON a.principal_id=p.id WHERE p.status='active' AND a.role='researcher' LIMIT 1")"
WORKER="$("${PSQL[@]}" "SELECT p.id FROM principals p JOIN agents a ON a.principal_id=p.id WHERE p.status='active' AND a.role='worker' LIMIT 1")"
VERIFIER="$("${PSQL[@]}" "SELECT p.id FROM principals p JOIN agents a ON a.principal_id=p.id WHERE p.status='active' AND a.role='verifier' LIMIT 1")"

for v in OWNER PLANNER RESEARCHER WORKER VERIFIER; do
  [ -n "${!v}" ] || { echo "FAIL: $v principal not found" >&2; exit 1; }
done

echo "=== BOOTSTRAP REFERENCE-DEMO CAPABILITIES ==="
docker compose exec -T postgres psql -U alp -d alp <<SQL >/dev/null
INSERT INTO capabilities(id,name,description) VALUES
('7eb95d4f-397d-4e3d-b040-58c747d15101','case.work','execute governed company work'),
('bd4e13e0-395d-4421-83c6-e087ff041902','case.supervise','independently supervise governed company work')
ON CONFLICT(name) DO NOTHING;

INSERT INTO principal_capabilities(principal_id,capability_id,granted_by)
SELECT p.id,c.id,'alp14-reference-gate'
FROM principals p CROSS JOIN capabilities c
WHERE p.id IN ('$PLANNER','$RESEARCHER','$WORKER','$VERIFIER') AND c.name='case.work'
ON CONFLICT DO NOTHING;

INSERT INTO principal_capabilities(principal_id,capability_id,granted_by)
SELECT '$VERIFIER',id,'alp14-reference-gate' FROM capabilities WHERE name='case.supervise'
ON CONFLICT DO NOTHING;

INSERT INTO principal_capabilities(principal_id,capability_id,granted_by)
SELECT '$OWNER',id,'alp14-reference-gate' FROM capabilities
WHERE name IN ('finance.configure','finance.post','finance.authorize_payment','finance.propose')
ON CONFLICT DO NOTHING;

INSERT INTO principal_capabilities(principal_id,capability_id,granted_by)
SELECT '$PLANNER',id,'alp14-reference-gate' FROM capabilities WHERE name='finance.propose'
ON CONFLICT DO NOTHING;
SQL

export ALP_BEAM_AGENTS="${PLANNER}|planner-01|planner;${RESEARCHER}|research-01|researcher;${WORKER}|worker-01|worker;${VERIFIER}|verifier-01|verifier"

echo "=== START GOVERNED BEAM WORKFORCE ==="
docker compose up -d --force-recreate beam-runtime
wait_http http://127.0.0.1:${ALP_BEAM_HOST_PORT:-8090}/health
curl -fsS http://127.0.0.1:${ALP_BEAM_HOST_PORT:-8090}/health | python3 -m json.tool

export ALP_OWNER_ID="$OWNER"
export ALP_PLANNER_ID="$PLANNER"
export ALP_RESEARCHER_ID="$RESEARCHER"
export ALP_WORKER_ID="$WORKER"
export ALP_VERIFIER_ID="$VERIFIER"
export ALP14_STAMP="$(date +%s)-$$"

echo "=== RUN FOUR END-TO-END REFERENCE COMPANIES ==="
python3 examples/reference_companies/run_demos.py | tee /tmp/alp14-reference-summary.txt

grep -q '^SUPPORT_SERVICE_DEMO_ASSERTION=PASS$' /tmp/alp14-reference-summary.txt
grep -q '^PROCUREMENT_ECONOMICS_DEMO_ASSERTION=PASS$' /tmp/alp14-reference-summary.txt
grep -q '^ENTERPRISE_TRANSITION_DEMO_ASSERTION=PASS$' /tmp/alp14-reference-summary.txt
grep -q '^PROJECT_DELIVERY_DEMO_ASSERTION=PASS$' /tmp/alp14-reference-summary.txt
grep -q '^REFERENCE_COMPANY_GENERALITY_ASSERTION=PASS$' /tmp/alp14-reference-summary.txt

echo "=== DURABLE DEMO EVIDENCE ==="
"${PSQL[@]}" "SELECT case_type,state,environment,COUNT(*) FROM company_cases WHERE created_at > now()-interval '10 minutes' GROUP BY case_type,state,environment ORDER BY case_type;"
"${PSQL[@]}" "SELECT COUNT(*) FROM simulation_runs WHERE created_at > now()-interval '10 minutes' AND mode='shadow' AND side_effects_executed=FALSE;"
"${PSQL[@]}" "SELECT COUNT(*) FROM payment_intents WHERE created_at > now()-interval '10 minutes' AND state='authorized' AND executed_at IS NULL;"

echo "ALP14_REFERENCE_COMPANIES_GATE=GREEN"
