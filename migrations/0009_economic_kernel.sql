INSERT INTO capabilities(id,name,description)
VALUES
  ('b7ecf1f4-7ef0-49a4-aea7-73fc535a3d01','finance.configure','configure governed economic records'),
  ('12dd1810-5e93-47c2-a6cb-06ec8995f5d8','finance.propose','propose governed commitments invoices and payments'),
  ('98de1e1d-2f8c-47ff-a1c4-ae8acb29f2ba','finance.post','post balanced governed journal entries'),
  ('6b14dbbc-b563-4b72-81ce-20ff3cb8f9ed','finance.authorize_payment','authorize governed payment intents')
ON CONFLICT(name) DO NOTHING;

CREATE TABLE IF NOT EXISTS economic_accounts (
  id UUID PRIMARY KEY,
  legal_entity TEXT NOT NULL DEFAULT 'default',
  code TEXT NOT NULL,
  name TEXT NOT NULL,
  account_type TEXT NOT NULL,
  currency TEXT NOT NULL,
  active BOOLEAN NOT NULL DEFAULT TRUE,
  created_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(legal_entity, code, currency)
);

CREATE INDEX IF NOT EXISTS idx_economic_accounts_entity_type
ON economic_accounts(legal_entity, account_type, active);

CREATE TABLE IF NOT EXISTS economic_budgets (
  id UUID PRIMARY KEY,
  case_id UUID NOT NULL REFERENCES company_cases(id) ON DELETE RESTRICT,
  amount_minor BIGINT NOT NULL CHECK (amount_minor >= 0),
  currency TEXT NOT NULL,
  period_start TIMESTAMPTZ,
  period_end TIMESTAMPTZ,
  state TEXT NOT NULL DEFAULT 'active',
  created_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(case_id, currency, state)
);

CREATE INDEX IF NOT EXISTS idx_economic_budgets_case_state
ON economic_budgets(case_id, state);

CREATE TABLE IF NOT EXISTS economic_commitments (
  id UUID PRIMARY KEY,
  case_id UUID NOT NULL REFERENCES company_cases(id) ON DELETE RESTRICT,
  description TEXT NOT NULL,
  counterparty_ref TEXT,
  amount_minor BIGINT NOT NULL CHECK (amount_minor > 0),
  currency TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'proposed',
  requested_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  approved_by_principal_id UUID REFERENCES principals(id) ON DELETE RESTRICT,
  approved_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_economic_commitments_case_state
ON economic_commitments(case_id, state);

CREATE TABLE IF NOT EXISTS economic_invoices (
  id UUID PRIMARY KEY,
  case_id UUID NOT NULL REFERENCES company_cases(id) ON DELETE RESTRICT,
  direction TEXT NOT NULL,
  counterparty_ref TEXT NOT NULL,
  amount_minor BIGINT NOT NULL CHECK (amount_minor > 0),
  currency TEXT NOT NULL,
  state TEXT NOT NULL,
  external_ref TEXT,
  created_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  approved_by_principal_id UUID REFERENCES principals(id) ON DELETE RESTRICT,
  approved_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_economic_invoices_case_state
ON economic_invoices(case_id, direction, state);

CREATE TABLE IF NOT EXISTS payment_intents (
  id UUID PRIMARY KEY,
  case_id UUID NOT NULL REFERENCES company_cases(id) ON DELETE RESTRICT,
  invoice_id UUID REFERENCES economic_invoices(id) ON DELETE RESTRICT,
  direction TEXT NOT NULL,
  amount_minor BIGINT NOT NULL CHECK (amount_minor > 0),
  currency TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'proposed',
  idempotency_key TEXT NOT NULL UNIQUE,
  requested_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  authorized_by_principal_id UUID REFERENCES principals(id) ON DELETE RESTRICT,
  authorized_at TIMESTAMPTZ,
  executed_at TIMESTAMPTZ,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_payment_intents_case_state
ON payment_intents(case_id, state);

CREATE TABLE IF NOT EXISTS journal_entries (
  id UUID PRIMARY KEY,
  case_id UUID NOT NULL REFERENCES company_cases(id) ON DELETE RESTRICT,
  description TEXT NOT NULL,
  currency TEXT NOT NULL,
  source_type TEXT,
  source_id UUID,
  idempotency_key TEXT NOT NULL UNIQUE,
  posted_by_principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
  posted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_journal_entries_case_posted
ON journal_entries(case_id, posted_at);

CREATE TABLE IF NOT EXISTS journal_lines (
  entry_id UUID NOT NULL REFERENCES journal_entries(id) ON DELETE CASCADE,
  line_no INTEGER NOT NULL,
  account_id UUID NOT NULL REFERENCES economic_accounts(id) ON DELETE RESTRICT,
  debit_minor BIGINT NOT NULL DEFAULT 0 CHECK (debit_minor >= 0),
  credit_minor BIGINT NOT NULL DEFAULT 0 CHECK (credit_minor >= 0),
  memo TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(entry_id, line_no),
  CHECK ((debit_minor > 0 AND credit_minor = 0) OR (credit_minor > 0 AND debit_minor = 0))
);

CREATE INDEX IF NOT EXISTS idx_journal_lines_account
ON journal_lines(account_id, entry_id);

CREATE TABLE IF NOT EXISTS economic_events (
  id UUID PRIMARY KEY,
  case_id UUID NOT NULL REFERENCES company_cases(id) ON DELETE CASCADE,
  principal_id UUID REFERENCES principals(id) ON DELETE SET NULL,
  kind TEXT NOT NULL,
  object_type TEXT NOT NULL,
  object_id UUID,
  payload_json TEXT NOT NULL DEFAULT '{}',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_economic_events_case_created
ON economic_events(case_id, created_at);
