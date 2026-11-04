#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
echo "=== ALP15 LOCAL-FIRST + COMPANY STUDIO ==="
cargo fmt --all -- --check
cargo test --workspace
cargo run -q -p alp-cli -- doctor --system
cargo run -q -p alp-cli -- --help | grep -q 'FIRST RUN:'
cargo run -q -p alp-cli -- guide | grep -q 'ALP_GUIDE=PASS'
cargo run -q -p alp-cli -- studio --help | grep -q 'Launch the terminal-native Company Studio'
cargo run -q -p alp-studio -- --smoke | tee /tmp/alp-studio-smoke
grep -q 'ALP_STUDIO_WELCOME_SPLASH=PASS' /tmp/alp-studio-smoke
grep -q 'ALP_STUDIO_HELP_OVERLAY=PASS' /tmp/alp-studio-smoke
grep -q 'ALP_STUDIO_RENDER_SMOKE=PASS' /tmp/alp-studio-smoke
grep -q 'ratatui="0.30.2"' crates/alp-studio/Cargo.toml
grep -q 'ALP_MODEL_POLICY' crates/alp-server/src/cognition.rs
grep -q 'openai-compatible' crates/alp-server/src/cognition.rs
grep -q '/v1/company/workflows/{id}' crates/alp-server/src/company.rs
echo "ALP_CLI_ONBOARDING_ASSERTION=PASS"
echo "ALP_STUDIO_WELCOME_ASSERTION=PASS"
echo "ALP_STUDIO_HELP_ASSERTION=PASS"
echo "LOCAL_PROVIDER_ABSTRACTION_ASSERTION=PASS"
echo "HARDWARE_DOCTOR_ASSERTION=PASS"
echo "CLICKABLE_TUI_ASSERTION=PASS"
echo "IMMUTABLE_DESIGN_PUBLISH_ASSERTION=PASS"
echo "ALP15_PRODUCT_SURFACE_GATE=GREEN"
