# Runtime Provider Contract

Every provider adapter should converge on:

- `health()`
- `capabilities()`
- `submit(workload)`
- `status(external_id)`
- `cancel(external_id)`
- `checkpoint(external_id)`
- `restore(checkpoint)`
- `logs(external_id)`
- `cost_estimate(workload)`

A provider acknowledging a request is not evidence that execution succeeded.

The control plane must keep distinct records for:

1. accepted
2. running
3. externally verified success/failure
