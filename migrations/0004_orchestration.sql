CREATE TABLE IF NOT EXISTS orchestrations (
  id UUID PRIMARY KEY,
  goal TEXT NOT NULL,
  state TEXT NOT NULL,
  current_seq INTEGER NOT NULL DEFAULT 1,
  created_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_orchestrations_state_updated
ON orchestrations(state, updated_at);

CREATE TABLE IF NOT EXISTS orchestration_steps (
  id UUID PRIMARY KEY,
  orchestration_id UUID NOT NULL REFERENCES orchestrations(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  role TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'pending',
  assigned_principal_id UUID REFERENCES principals(id) ON DELETE SET NULL,
  attempt INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 3,
  input_json TEXT NOT NULL DEFAULT '{}',
  output_json TEXT,
  claimed_at TIMESTAMPTZ,
  lease_expires_at TIMESTAMPTZ,
  completed_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(orchestration_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_orchestration_steps_claim
ON orchestration_steps(role, state, seq, created_at);

CREATE INDEX IF NOT EXISTS idx_orchestration_steps_lease
ON orchestration_steps(state, lease_expires_at);

CREATE TABLE IF NOT EXISTS orchestration_handoffs (
  id UUID PRIMARY KEY,
  orchestration_id UUID NOT NULL REFERENCES orchestrations(id) ON DELETE CASCADE,
  from_step_id UUID NOT NULL REFERENCES orchestration_steps(id) ON DELETE CASCADE,
  to_step_id UUID NOT NULL REFERENCES orchestration_steps(id) ON DELETE CASCADE,
  from_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  to_principal_id UUID REFERENCES principals(id) ON DELETE SET NULL,
  payload_json TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_orchestration_handoffs_flow
ON orchestration_handoffs(orchestration_id, created_at);

CREATE TABLE IF NOT EXISTS orchestration_events (
  id UUID PRIMARY KEY,
  orchestration_id UUID NOT NULL REFERENCES orchestrations(id) ON DELETE CASCADE,
  step_id UUID REFERENCES orchestration_steps(id) ON DELETE SET NULL,
  principal_id UUID REFERENCES principals(id) ON DELETE SET NULL,
  kind TEXT NOT NULL,
  message TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_orchestration_events_created
ON orchestration_events(orchestration_id, created_at);
