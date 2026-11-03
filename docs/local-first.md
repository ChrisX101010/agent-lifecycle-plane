# Local-first operation

Agent Lifecycle Plane v0.1 is designed to start on one machine and preserve the same governed company/workflow model as infrastructure grows.

## Model providers

The implemented cognition surface supports:

- `ollama`
- `openai-compatible` local endpoints such as LM Studio

## Model policies

- `local_only` — only loopback/localhost/`host.docker.internal` model endpoints are accepted
- `hybrid` — reserved for deployments that deliberately combine local and approved remote providers
- `cloud_allowed` — remote providers may be configured by the operator

The release default is `local_only`.

## System check

```bash
cargo run -p alp-cli -- doctor --system
```

`doctor` reports local prerequisites, visible RAM/GPU/VRAM, Ollama availability/models, and a conservative deployment-profile recommendation.

## Network boundary

Compose publishes the API, PostgreSQL, and BEAM runtime on `127.0.0.1` by default. `ALP_SECURITY_MODE=local` relies on that boundary and does not authenticate individual local users.

Read `SECURITY.md` before changing bind addresses or forwarding ports.

## Growth path

The data model can represent broader runtime/provider classes, but v0.1 does not claim production Kubernetes, HA, multi-tenant, or public-cloud deployment readiness. Those are separate deployment hardening stages rather than assumptions hidden inside the core workflow model.
