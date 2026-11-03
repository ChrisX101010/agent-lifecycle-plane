# Changelog

All notable release-level changes to Agent Lifecycle Plane are documented here.

## 0.1.0-beta — release candidate

### Added

- Rust control plane with PostgreSQL durable state
- capability-governed task and approval model
- BEAM/OTP supervised agent workforce
- durable orchestration, retries, leases, reconciliation, and verification
- governed web evidence tooling and local model-provider abstraction
- generic company workflow/case/work-item kernel
- portable blueprints, canonical mappings, connectors, and sandbox/shadow replay
- project resources and workplace performance signals
- economic kernel with budgets, commitments, invoices, payment intents, and balanced journals
- terminal-native Company Studio
- four end-to-end reference-company scenarios

### Security / release hardening

- loopback-only host publishing by default
- local/token API security modes
- non-permissive CORS and 1 MiB request limit
- local-only model endpoint enforcement
- staged-tree secret scanning and RustSec dependency audit
- PostgreSQL-only SQLx compatibility surface to avoid unused vulnerable database-driver dependencies

The release remains a local-first, single-tenant beta. See `SECURITY.md` for explicit non-goals.
