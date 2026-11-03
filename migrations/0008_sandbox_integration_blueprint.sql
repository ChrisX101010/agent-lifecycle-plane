CREATE TABLE IF NOT EXISTS company_blueprints (
  id UUID PRIMARY KEY,
  name TEXT NOT NULL,
  version INTEGER NOT NULL,
  classification TEXT NOT NULL DEFAULT 'confidential',
  source_workflow_definition_id UUID REFERENCES workflow_definitions(id) ON DELETE SET NULL,
  package_json TEXT NOT NULL,
  package_sha256 TEXT NOT NULL,
  imported_from_sha256 TEXT,
  created_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(name, version)
);

CREATE INDEX IF NOT EXISTS idx_company_blueprints_name_version
ON company_blueprints(name, version DESC);

CREATE TABLE IF NOT EXISTS blueprint_mappings (
  id UUID PRIMARY KEY,
  blueprint_id UUID NOT NULL REFERENCES company_blueprints(id) ON DELETE CASCADE,
  domain TEXT NOT NULL,
  canonical_type TEXT NOT NULL,
  target_system TEXT NOT NULL,
  target_type TEXT NOT NULL,
  owner_system TEXT NOT NULL,
  direction TEXT NOT NULL,
  mapping_json TEXT NOT NULL DEFAULT '{}',
  created_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(blueprint_id, domain, canonical_type, target_system, target_type)
);

CREATE INDEX IF NOT EXISTS idx_blueprint_mappings_blueprint
ON blueprint_mappings(blueprint_id, domain, canonical_type);

CREATE TABLE IF NOT EXISTS connector_registrations (
  id UUID PRIMARY KEY,
  name TEXT NOT NULL,
  provider TEXT NOT NULL,
  transport TEXT NOT NULL,
  environment TEXT NOT NULL,
  mode TEXT NOT NULL,
  credential_ref TEXT,
  capabilities_json TEXT NOT NULL DEFAULT '[]',
  config_json TEXT NOT NULL DEFAULT '{}',
  state TEXT NOT NULL DEFAULT 'active',
  created_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(name, environment)
);

CREATE INDEX IF NOT EXISTS idx_connector_registrations_environment
ON connector_registrations(environment, state, provider);

CREATE TABLE IF NOT EXISTS simulation_runs (
  id UUID PRIMARY KEY,
  source_case_id UUID NOT NULL REFERENCES company_cases(id) ON DELETE RESTRICT,
  target_blueprint_id UUID NOT NULL REFERENCES company_blueprints(id) ON DELETE RESTRICT,
  mode TEXT NOT NULL,
  state TEXT NOT NULL,
  process_equivalence DOUBLE PRECISION NOT NULL,
  side_effects_executed BOOLEAN NOT NULL DEFAULT FALSE,
  result_json TEXT NOT NULL DEFAULT '{}',
  requested_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_simulation_runs_target_created
ON simulation_runs(target_blueprint_id, created_at DESC);
