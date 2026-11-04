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
- terminal-native Company Studio with animated ASCII welcome screen and in-product help overlay
- guided `alpctl` onboarding, Studio launcher, demo command, and release-check command
- four end-to-end reference-company scenarios

### Security / release hardening

- loopback-only host publishing by default
- local/token API security modes
- non-permissive CORS and 1 MiB request limit
- local-only model endpoint enforcement
- staged-tree secret scanning and RustSec dependency audit
- PostgreSQL-only SQLx compatibility surface to avoid unused vulnerable database-driver dependencies
- non-root BEAM runtime plus Docker no-new-privileges/capability-drop assertions
- runtime checks for request-body limits and deny-by-default CORS behavior
- internal HTTP 500 error redaction with server-side logging
- Ratatui render smoke coverage for the welcome splash and help overlay

The release remains a local-first, single-tenant beta. See `SECURITY.md` for explicit non-goals.
