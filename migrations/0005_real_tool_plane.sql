CREATE TABLE IF NOT EXISTS tool_calls (
  id UUID PRIMARY KEY,
  orchestration_id UUID NOT NULL REFERENCES orchestrations(id) ON DELETE CASCADE,
  principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  tool_name TEXT NOT NULL,
  request_json TEXT NOT NULL,
  state TEXT NOT NULL,
  http_status INTEGER,
  response_sha256 TEXT,
  error TEXT,
  started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_tool_calls_orchestration_started
ON tool_calls(orchestration_id, started_at);

CREATE TABLE IF NOT EXISTS evidence_artifacts (
  id UUID PRIMARY KEY,
  orchestration_id UUID NOT NULL REFERENCES orchestrations(id) ON DELETE CASCADE,
  tool_call_id UUID NOT NULL REFERENCES tool_calls(id) ON DELETE RESTRICT,
  principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  source_url TEXT NOT NULL,
  content_type TEXT,
  content_sha256 TEXT NOT NULL,
  byte_length BIGINT NOT NULL,
  content TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_evidence_orchestration_created
ON evidence_artifacts(orchestration_id, created_at);

CREATE TABLE IF NOT EXISTS work_artifacts (
  id UUID PRIMARY KEY,
  orchestration_id UUID NOT NULL REFERENCES orchestrations(id) ON DELETE CASCADE,
  evidence_id UUID NOT NULL REFERENCES evidence_artifacts(id) ON DELETE RESTRICT,
  principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  media_type TEXT NOT NULL,
  content_sha256 TEXT NOT NULL,
  content TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_work_artifacts_orchestration_created
ON work_artifacts(orchestration_id, created_at);

CREATE TABLE IF NOT EXISTS artifact_verifications (
  id UUID PRIMARY KEY,
  orchestration_id UUID NOT NULL REFERENCES orchestrations(id) ON DELETE CASCADE,
  artifact_id UUID NOT NULL REFERENCES work_artifacts(id) ON DELETE CASCADE,
  verifier_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  decision TEXT NOT NULL,
  reason TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_artifact_verifications_orchestration_created
ON artifact_verifications(orchestration_id, created_at);
