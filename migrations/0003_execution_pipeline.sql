ALTER TABLE tasks
ADD COLUMN IF NOT EXISTS created_by_principal_id UUID
REFERENCES principals(id) ON DELETE SET NULL;

ALTER TABLE audit_events
ADD COLUMN IF NOT EXISTS principal_id UUID
REFERENCES principals(id) ON DELETE SET NULL;

CREATE TABLE IF NOT EXISTS task_leases (
  id UUID PRIMARY KEY,
  task_id UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  worker_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  authorized_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  state TEXT NOT NULL,
  acquired_at TIMESTAMPTZ NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL,
  released_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_task_active_lease
ON task_leases(task_id)
WHERE state='active';

CREATE INDEX IF NOT EXISTS idx_task_leases_worker_state
ON task_leases(worker_principal_id,state,expires_at);

CREATE TABLE IF NOT EXISTS execution_receipts (
  id UUID PRIMARY KEY,
  task_id UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  worker_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  runtime TEXT NOT NULL,
  external_id TEXT NOT NULL,
  status TEXT NOT NULL,
  accepted_at TIMESTAMPTZ NOT NULL,
  completed_at TIMESTAMPTZ,
  output_summary TEXT,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_execution_receipts_task_created
ON execution_receipts(task_id,created_at);

CREATE TABLE IF NOT EXISTS verifications (
  id UUID PRIMARY KEY,
  task_id UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  receipt_id UUID NOT NULL REFERENCES execution_receipts(id) ON DELETE CASCADE,
  verifier_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  decision TEXT NOT NULL,
  reason TEXT,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_task_verification
ON verifications(task_id);

CREATE INDEX IF NOT EXISTS idx_verifications_verifier_created
ON verifications(verifier_principal_id,created_at);
