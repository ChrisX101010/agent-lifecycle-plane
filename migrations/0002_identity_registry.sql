CREATE TABLE IF NOT EXISTS principals (
  id UUID PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'active',
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS agents (
  principal_id UUID PRIMARY KEY
    REFERENCES principals(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  runtime TEXT NOT NULL,
  model_provider TEXT,
  model_name TEXT,
  routing_strategy TEXT NOT NULL,
  monthly_budget_eur DOUBLE PRECISION,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS clients (
  principal_id UUID PRIMARY KEY
    REFERENCES principals(id) ON DELETE CASCADE,
  client_type TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS capabilities (
  id UUID PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS principal_capabilities (
  principal_id UUID NOT NULL
    REFERENCES principals(id) ON DELETE CASCADE,
  capability_id UUID NOT NULL
    REFERENCES capabilities(id) ON DELETE CASCADE,
  granted_by TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (principal_id, capability_id)
);

CREATE TABLE IF NOT EXISTS registry_events (
  id UUID PRIMARY KEY,
  subject_type TEXT NOT NULL,
  subject_id UUID,
  kind TEXT NOT NULL,
  message TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_principals_kind_status
ON principals(kind, status);

CREATE INDEX IF NOT EXISTS idx_agents_role
ON agents(role);

CREATE INDEX IF NOT EXISTS idx_registry_events_subject_created
ON registry_events(subject_type, subject_id, created_at);
