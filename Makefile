.PHONY: bootstrap dev test fmt lint check audit compose floci release-check public-check

bootstrap:
	./bootstrap.sh

dev:
	cargo run -p alp-server

test:
	cargo test --workspace --locked

fmt:
	cargo fmt --all -- --check

lint:
	cargo clippy --workspace --all-targets --locked -- -D warnings

check: fmt lint test

audit:
	cargo audit
	python3 scripts/secret_scan.py

compose:
	docker compose up --build

floci:
	docker compose --profile cloud-emulation up

release-check:
	bash scripts/alp17_release_candidate_gate.sh

public-check:
	bash scripts/public_release_gate.sh
