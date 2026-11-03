CREATE TABLE IF NOT EXISTS model_calls (
  id UUID PRIMARY KEY,
  orchestration_id UUID NOT NULL REFERENCES orchestrations(id) ON DELETE CASCADE,
  principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  role TEXT NOT NULL,
  purpose TEXT NOT NULL,
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  prompt_sha256 TEXT NOT NULL,
  response_sha256 TEXT,
  state TEXT NOT NULL,
  response_text TEXT,
  error TEXT,
  started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_model_calls_orchestration_started
ON model_calls(orchestration_id, started_at);

CREATE INDEX IF NOT EXISTS idx_model_calls_principal_started
ON model_calls(principal_id, started_at);

ALTER TABLE work_artifacts
ADD COLUMN IF NOT EXISTS model_call_id UUID REFERENCES model_calls(id) ON DELETE SET NULL;
