# v0.1 release candidate checklist

A release is not ready because it builds once. The first public commit/tag should satisfy both product and publication gates.

## Product gate

```bash
bash scripts/alp17_release_candidate_gate.sh
```

This covers the local-first surface, security assertions, Rust quality checks, RustSec dependency audit, and the four reference-company regressions.

## Public staged-tree gate

```bash
bash scripts/public_release_gate.sh
```

This checks the exact Git index for:

- forbidden local/private paths
- accidental `.env` / key / backup files
- absolute home-directory traces
- built-in secret findings
- Gitleaks findings
- whitespace/index corruption
- required public documentation

## Before the first tag

1. Review `git status --short`.
2. Review `git diff --cached --stat` and the staged diff.
3. Confirm `.env`, credentials, customer/vendor data, backups, database dumps, and private blueprints are absent.
4. Keep public reference-company data synthetic.
5. Run both gates from a clean clone before tagging.
6. Do not describe v0.1 as multi-tenant, internet-hardened, compliance-certified, or production Kubernetes-ready.
