# Security policy

## Supported scope

Agent Lifecycle Plane v0.1 is a **local-first, single-tenant beta**. It is not an internet-facing multi-tenant service and must not be presented or deployed as one without additional hardening.

## Default boundary

- Docker-published API, PostgreSQL, and BEAM ports bind to loopback by default.
- `ALP_SECURITY_MODE=local` performs no user authentication. Its security boundary is the local host / trusted container boundary.
- `ALP_SECURITY_MODE=token` requires a bearer token of at least 32 characters for API routes other than `/health`.
- A non-loopback API bind requires the explicit trusted-container boundary flag used by the local Docker deployment.
- Permissive CORS is not enabled.
- Request bodies are capped at 1 MiB.
- `ALP_MODEL_POLICY=local_only` rejects non-local model endpoints.

Do not publish ports `8080`, `8090`, `5432`, `4566`, or `4577` to an untrusted network.

## Secrets

- `.env` is local-only and ignored by Git.
- `bootstrap.sh` generates a random PostgreSQL password on first run and does not print it.
- API/model credentials must come from environment variables or an external secret store.
- Connector records store secret references (`vault://` or `secret://`), not inline credentials.
- Never commit customer/vendor credentials, production blueprints, database dumps, private keys, or exported environment files.

## Model and tool authority

- Model cognition is advisory; it cannot authorize itself.
- Research fetches are HTTPS-only, allowlisted by hostname, redirect-disabled, size-limited, and textual-content-only.
- Only trusted public domains should be added to `ALP_WEB_ALLOWLIST`.
- External content is evidence, never authorization.
- Sandbox/shadow replay must not execute external side effects.
- Payment execution is intentionally external to the v0.1 economic kernel.

## BEAM management surface

The BEAM runtime management HTTP surface is intended for the local operator/runtime only. It is loopback-published by Compose and is not hardened for hostile local peers. Do not expose it publicly.

## Optional cloud emulation

The `cloud-emulation` Compose profile mounts the Docker socket into Floci containers. Access to the Docker socket is highly privileged. Use that profile only with trusted images on a trusted development machine. Release tags are pinned in `docker-compose.yml` for reproducibility.

## Dependency and release checks

The release process includes:

- `cargo fmt --check`
- `cargo clippy -- -D warnings`
- workspace tests
- RustSec `cargo audit`
- built-in secret scanning
- Gitleaks over the exact staged release tree
- forbidden-file and personal-path checks
- end-to-end reference-company regression gates

Run:

```bash
bash scripts/alp17_release_candidate_gate.sh
bash scripts/public_release_gate.sh
```

## Not claimed in v0.1

- hostile multi-user local isolation
- multi-tenant authorization / ABAC
- enterprise SSO / SCIM
- TLS termination or public reverse-proxy configuration
- production secret-manager integration
- HA / backup / restore guarantees
- image signing / SLSA provenance
- regulatory certification
- HSM-backed financial execution

## Reporting a vulnerability

Do not publish exploit details in a public issue. Use GitHub private vulnerability reporting / a private security advisory when available. If the repository is private, contact the maintainer through the repository's private collaboration channel.
