# Agent Lifecycle Plane

> **Governed autonomy for real work.**
>
> Run agent-powered business workflows without giving models the keys.

Agent Lifecycle Plane is a **local-first control plane for governed autonomous work**. It keeps business process, authority, evidence, supervision, economics, and runtime state outside the model so that an LLM can propose actions without becoming the authority that approves or verifies them.

The v0.1 line is intentionally narrow: **single-tenant, local-first, and operator-controlled**. It is built for experimentation, solopreneurs, startups, small teams, and controlled internal environments—not public-internet multi-tenant SaaS.

## Why Agent Lifecycle Plane

Most agent frameworks focus on what an agent can do. Agent Lifecycle Plane focuses on:

- **who** is allowed to do it;
- **under what authority**;
- **against which durable workflow and budget**;
- **with what evidence and audit trail**;
- **who independently verifies the result**; and
- **what happens when work fails, stalls, exceeds policy, or needs escalation**.

The core rule is simple:

> **Models propose. Deterministic policy and independent verification authorize.**

## What is implemented in v0.1

- Rust control plane with PostgreSQL durable state
- BEAM/OTP supervised agent workforce and crash recovery
- capability-based principals and governed task execution
- durable orchestration, leases, retries, reconciliation, and verification
- governed HTTPS research tools with allowlists, evidence hashing, size limits, and redirect denial
- local cognition through Ollama and OpenAI-compatible local endpoints
- generic workflow / case / work-item company kernel
- project dates, participants, resources, budgets, and supervisor scoring
- agent workplace performance events
- portable company blueprints with redacted/public export modes
- sandbox/shadow replay with side effects disabled
- connector and source-of-truth mappings
- budgets, commitments, invoices, payment intents, and balanced journal entries
- terminal-native Company Studio built with Ratatui
- four end-to-end reference-company scenarios

## Architecture

```mermaid
flowchart LR
    U[Operator / Business Workflow] --> CP[Rust Control Plane]
    M[Local Model Provider] -->|advisory proposals| CP
    CP --> DB[(PostgreSQL durable state)]
    CP --> B[BEAM / OTP Agent Workforce]
    B --> CP
    CP --> T[Governed Tools / Connectors]
    T --> E[Evidence + Artifacts]
    E --> CP
    CP --> V[Independent Verification]
    V --> CP
    CP --> K[Case / Project / Economic Kernel]
```

The model is never the source of authority. Durable state, policy checks, leases, evidence, supervisor review, and economic controls stay in the control plane.

See [`docs/architecture.md`](docs/architecture.md) for the detailed boundary model.

## Quick start

### Prerequisites

- Docker with Compose v2
- Rust toolchain with `cargo`
- Python 3
- `curl`
- optional: Ollama for local cognition

### Start the local control plane

```bash
./bootstrap.sh
```

`bootstrap.sh` creates a private `.env` on first run and generates a random PostgreSQL password. It never prints that password.

Check the machine and local provider setup:

```bash
cargo run -p alp-cli -- doctor --system
```

The CLI executable is named `alpctl` (the Cargo package remains `alp-cli`) to avoid colliding with unrelated tools that already use the short `alp` command.

Launch the terminal Company Studio:

```bash
cargo run -p alp-studio
```

By default, published host ports bind to `127.0.0.1`, security mode is local, and model policy is `local_only`.

## Reference companies

The same kernel is exercised against four materially different workflows:

| Scenario | What the gate proves |
| --- | --- |
| Managed support | governed delivery, supervision, revenue, cost, margin |
| Procurement | budget, commitment, AP, payment-intent authority separation |
| Enterprise transition | blueprint export/import, canonical mappings, shadow replay, no side effects |
| Project delivery | dates, participants, parallel work, budget, revenue, expense, margin |

Run them with:

```bash
bash scripts/alp14_reference_companies_gate.sh
```

See [`docs/reference-companies.md`](docs/reference-companies.md).

## Release gates

The product release candidate gate combines tests, Clippy, dependency audit, secret hygiene, security assertions, and the reference-company regression suite:

```bash
bash scripts/alp17_release_candidate_gate.sh
```

Before a public commit or tag, run the staged-tree gate as well:

```bash
bash scripts/public_release_gate.sh
```

A release should not be tagged unless both gates finish GREEN.

## Security posture

v0.1 is a **local-first, single-tenant beta**. Local mode intentionally relies on loopback/container-boundary isolation rather than user authentication. Token mode is available for controlled local deployments, but this project does **not** claim public-internet hardening, multi-tenant isolation, enterprise SSO, regulatory certification, or hardened payment execution.

Read [`SECURITY.md`](SECURITY.md) before exposing any service beyond localhost.

## Local model policy

Supported provider surfaces:

- Ollama
- OpenAI-compatible local endpoints such as LM Studio

Supported policy modes:

- `local_only`
- `hybrid`
- `cloud_allowed`

`local_only` rejects non-local model endpoints in the control plane.

## Existing enterprise systems

Agent Lifecycle Plane is designed to sit **above** transport/vendor-specific systems rather than replace them. SAP, Salesforce, ServiceNow, Oracle, MuleSoft, Boomi, and similar systems can remain systems of record or integration transports while Agent Lifecycle Plane owns governed workflow, authority, evidence, supervision, economics, and replay semantics.

Product names and trademarks belong to their respective owners; no affiliation is implied.

## Scope of this beta

This repository deliberately does **not** claim:

- public-internet production safety
- multi-tenant authorization or tenant isolation
- SSO / SCIM / enterprise IAM
- high availability or disaster recovery
- HSM-backed payment execution
- compliance or regulatory certification
- production Kubernetes packaging
- a production web dashboard

Those boundaries are intentional. The first release focuses on making the governed execution kernel small enough to inspect and strong enough to test end-to-end.

## License

This source release is **not open source**. No license is granted beyond inspection/evaluation unless a separate written license says otherwise. See [`LICENSE`](LICENSE).
