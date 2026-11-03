# ALP reference companies

ALP14 proves that the same governed company kernel can run materially different business domains without introducing a domain-specific runtime for each one.

## Reference scenarios

### Managed service / support company

A confidential customer request becomes a durable `support` case. Planner intake, research diagnosis, delivery work and independent supervisor review all run through the ALP company loop. The same case is linked to AR/AP records and balanced journals so the service has measurable revenue, delivery cost and margin.

### Procurement / trading company

A governed procurement case moves from request to sourcing, supplier selection and supervisor review. The economic kernel separately enforces budget, commitment, payable invoice and payment-intent authority. The demo deliberately stops at an authorized payment intent; external money movement remains outside the economic kernel.

### Enterprise transition

A completed migration case is exported as a confidential portable blueprint. SAP and Salesforce-style records are mapped through canonical business types with explicit `owner_system` boundaries. A MuleSoft-style connector is registered as transport-only, then the process is replayed in `shadow` mode. Replay is simulation-only and must execute no side effects.

### Project delivery

A project case carries start/end dates, budget, four agent allocations, parallel requirements/risk work, delivery and supervisor review. Revenue and delivery cost are posted to the same project case to produce budget, revenue, expense and margin views.

## Architectural point

These are not four separate applications. They share the same primitives:

- workflow definition
- case
- work item
- principal / capability
- supervisor review
- evidence / audit events
- blueprint / mapping / connector
- budget / commitment / invoice / payment intent / ledger

That is the intended product boundary: business domains define workflows and records; the ALP kernel provides governed execution, continuity, economics, simulation and authority boundaries.

## Run

```bash
bash scripts/alp14_reference_companies_gate.sh
```

The gate boots the API and BEAM workforce, runs all four scenarios end-to-end and fails if any reference-company invariant is violated.
