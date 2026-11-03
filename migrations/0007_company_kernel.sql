CREATE TABLE IF NOT EXISTS workflow_definitions (
  id UUID PRIMARY KEY,
  name TEXT NOT NULL,
  version INTEGER NOT NULL,
  case_type TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'published',
  definition_json TEXT NOT NULL,
  created_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(name, version)
);

CREATE INDEX IF NOT EXISTS idx_workflow_definitions_case_type
ON workflow_definitions(case_type, state, version);

CREATE TABLE IF NOT EXISTS company_cases (
  id UUID PRIMARY KEY,
  case_type TEXT NOT NULL,
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  workflow_definition_id UUID NOT NULL REFERENCES workflow_definitions(id) ON DELETE RESTRICT,
  workflow_version INTEGER NOT NULL,
  environment TEXT NOT NULL DEFAULT 'sandbox',
  state TEXT NOT NULL DEFAULT 'running',
  priority INTEGER NOT NULL DEFAULT 50,
  budget_amount DOUBLE PRECISION,
  budget_currency TEXT,
  planned_start_at TIMESTAMPTZ,
  planned_end_at TIMESTAMPTZ,
  actual_start_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  actual_end_at TIMESTAMPTZ,
  created_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_company_cases_state_priority
ON company_cases(state, priority DESC, created_at);

CREATE TABLE IF NOT EXISTS case_participants (
  case_id UUID NOT NULL REFERENCES company_cases(id) ON DELETE CASCADE,
  principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  assignment_role TEXT NOT NULL,
  allocation_percent DOUBLE PRECISION NOT NULL DEFAULT 100.0,
  assigned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY(case_id, principal_id, assignment_role)
);

CREATE TABLE IF NOT EXISTS case_work_items (
  id UUID PRIMARY KEY,
  case_id UUID NOT NULL REFERENCES company_cases(id) ON DELETE CASCADE,
  node_key TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'task',
  role TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'blocked',
  required BOOLEAN NOT NULL DEFAULT TRUE,
  attempt INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 3,
  depends_on_json TEXT NOT NULL DEFAULT '[]',
  condition_json TEXT,
  input_json TEXT NOT NULL DEFAULT '{}',
  output_json TEXT,
  checklist_json TEXT NOT NULL DEFAULT '[]',
  assigned_principal_id UUID REFERENCES principals(id) ON DELETE SET NULL,
  claimed_at TIMESTAMPTZ,
  lease_expires_at TIMESTAMPTZ,
  available_at TIMESTAMPTZ,
  due_at TIMESTAMPTZ,
  completed_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(case_id, node_key)
);

CREATE INDEX IF NOT EXISTS idx_case_work_items_claim
ON case_work_items(role, state, available_at, created_at);

CREATE INDEX IF NOT EXISTS idx_case_work_items_lease
ON case_work_items(state, lease_expires_at);

CREATE TABLE IF NOT EXISTS case_events (
  id UUID PRIMARY KEY,
  case_id UUID NOT NULL REFERENCES company_cases(id) ON DELETE CASCADE,
  work_item_id UUID REFERENCES case_work_items(id) ON DELETE SET NULL,
  principal_id UUID REFERENCES principals(id) ON DELETE SET NULL,
  kind TEXT NOT NULL,
  message TEXT NOT NULL,
  payload_json TEXT NOT NULL DEFAULT '{}',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_case_events_case_created
ON case_events(case_id, created_at);

CREATE TABLE IF NOT EXISTS case_reviews (
  id UUID PRIMARY KEY,
  case_id UUID NOT NULL REFERENCES company_cases(id) ON DELETE CASCADE,
  work_item_id UUID REFERENCES case_work_items(id) ON DELETE SET NULL,
  reviewer_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  score DOUBLE PRECISION NOT NULL,
  decision TEXT NOT NULL,
  dimensions_json TEXT NOT NULL DEFAULT '{}',
  notes TEXT NOT NULL DEFAULT '',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_case_reviews_case_created
ON case_reviews(case_id, created_at);

CREATE TABLE IF NOT EXISTS agent_performance_events (
  id UUID PRIMARY KEY,
  principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  case_id UUID NOT NULL REFERENCES company_cases(id) ON DELETE CASCADE,
  work_item_id UUID NOT NULL REFERENCES case_work_items(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  outcome TEXT NOT NULL,
  duration_ms BIGINT,
  attempts INTEGER NOT NULL,
  quality_score DOUBLE PRECISION,
  sla_met BOOLEAN,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_agent_performance_principal_created
ON agent_performance_events(principal_id, created_at);
