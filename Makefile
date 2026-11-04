.PHONY: bootstrap dev guide studio demo test fmt lint check audit compose floci release-check public-check

bootstrap:
	./bootstrap.sh

dev:
	cargo run -p alp-server

guide:
	cargo run -p alp-cli -- guide

studio:
	cargo run -p alp-studio

demo:
	bash scripts/alp14_reference_companies_gate.sh

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
