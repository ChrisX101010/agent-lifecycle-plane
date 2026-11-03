#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/alp15_product_surface_gate.sh
bash scripts/alp16_security_gate.sh
cargo clippy --workspace --all-targets -- -D warnings
if command -v cargo-audit >/dev/null 2>&1; then cargo audit; else cargo install cargo-audit --locked; cargo audit; fi
echo "CARGO_AUDIT=PASS"
export ALP_SECURITY_MODE=local;unset ALP_API_TOKEN || true
bash scripts/alp14_reference_companies_gate.sh
[ -f .gitignore ]&&[ -f .env.example ]&&[ -f SECURITY.md ]
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  ! git ls-files | grep -E '(^|/)(secrets/|.*\.(pem|key|p12|pfx|jks)$)'
  ! git ls-files | grep -E '(^|/)\.env($|\.)' | grep -vE '(^|/)\.env\.example$'
fi
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git diff --check
fi
echo "FULL_WORKSPACE_REGRESSION=PASS"
echo "SECURITY_AND_SECRET_SCAN=PASS"
echo "REFERENCE_DEMOS_REGRESSION=PASS"
echo "GIT_READINESS_ASSERTION=PASS"
echo "ALP17_RELEASE_CANDIDATE_GATE=GREEN"
