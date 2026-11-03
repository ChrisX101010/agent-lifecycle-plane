#!/usr/bin/env python3
import json
import os
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

BASE = os.environ.get("ALP_API", "http://127.0.0.1:8080").rstrip("/")
OWNER = os.environ["ALP_OWNER_ID"]
PLANNER = os.environ["ALP_PLANNER_ID"]
RESEARCHER = os.environ["ALP_RESEARCHER_ID"]
WORKER = os.environ["ALP_WORKER_ID"]
VERIFIER = os.environ["ALP_VERIFIER_ID"]
STAMP = os.environ.get("ALP14_STAMP", str(int(time.time())))
ROOT = Path(__file__).resolve().parent
WORKFLOWS = ROOT / "workflows"


def api(method, path, body=None, expect=(200, 201)):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(
        BASE + path,
        data=data,
        method=method,
        headers={"content-type": "application/json"} if data is not None else {},
    )
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            raw = r.read().decode()
            if r.status not in expect:
                raise RuntimeError(f"{method} {path}: HTTP {r.status}: {raw}")
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        raw = e.read().decode(errors="replace")
        raise RuntimeError(f"{method} {path}: HTTP {e.code}: {raw}") from e


def workflow(case_type, file_name):
    definition = json.loads((WORKFLOWS / file_name).read_text())
    return api("POST", "/v1/company/workflows", {
        "name": f"alp14-{case_type}-{STAMP}",
        "version": 1,
        "case_type": case_type,
        "created_by_principal_id": OWNER,
        "definition": definition,
    })


def create_case(case_type, wf_id, title, *, budget=None, start=None, end=None, metadata=None):
    body = {
        "case_type": case_type,
        "title": title,
        "description": f"ALP14 reference company demo: {case_type}",
        "workflow_definition_id": wf_id,
        "created_by_principal_id": OWNER,
        "environment": "sandbox",
        "priority": 80,
        "metadata": metadata or {"classification": "confidential", "demo": "alp14"},
    }
    if budget is not None:
        body["budget_amount"] = budget
        body["budget_currency"] = "EUR"
    if start:
        body["planned_start_at"] = start
    if end:
        body["planned_end_at"] = end
    return api("POST", "/v1/company/cases", body)


def add_participants(case_id):
    entries = [
        (PLANNER, "manager", 50.0),
        (RESEARCHER, "analyst", 60.0),
        (WORKER, "delivery", 100.0),
        (VERIFIER, "supervisor", 25.0),
    ]
    for principal, role, allocation in entries:
        api("POST", f"/v1/company/cases/{case_id}/participants", {
            "principal_id": principal,
            "assignment_role": role,
            "allocation_percent": allocation,
            "authorized_by_principal_id": OWNER,
        })


def wait_case(case_id, timeout=60):
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        last = api("GET", f"/v1/company/cases/{case_id}")
        state = last["case"]["state"]
        if state in {"completed", "needs_attention", "failed"}:
            return last
        time.sleep(0.4)
    raise RuntimeError(f"case {case_id} did not finish; last={last}")


def create_account(entity, code, name, kind):
    return api("POST", "/v1/company/economics/accounts", {
        "legal_entity": entity,
        "code": code,
        "name": name,
        "account_type": kind,
        "currency": "EUR",
        "created_by_principal_id": OWNER,
    })


def create_budget(case_id, amount_minor):
    return api("POST", "/v1/company/economics/budgets", {
        "case_id": case_id,
        "amount_minor": amount_minor,
        "currency": "EUR",
        "created_by_principal_id": OWNER,
    })


def invoice(case_id, direction, counterparty, amount, ref):
    return api("POST", "/v1/company/economics/invoices", {
        "case_id": case_id,
        "direction": direction,
        "counterparty_ref": counterparty,
        "amount_minor": amount,
        "currency": "EUR",
        "external_ref": ref,
        "created_by_principal_id": PLANNER,
    })


def journal(case_id, desc, lines, key):
    return api("POST", "/v1/company/economics/journals", {
        "case_id": case_id,
        "description": desc,
        "currency": "EUR",
        "source_type": "alp14_demo",
        "idempotency_key": f"alp14-{STAMP}-{key}",
        "posted_by_principal_id": OWNER,
        "lines": lines,
    })


def economics(case_id):
    return api("GET", f"/v1/company/economics/cases/{case_id}/summary")


# Shared chart-of-accounts for the reference-company suite.
entity = f"ALP14-{STAMP}"
AR = create_account(entity, "1100", "Accounts Receivable", "asset")["id"]
AP = create_account(entity, "2100", "Accounts Payable", "liability")["id"]
REV = create_account(entity, "4000", "Service Revenue", "revenue")["id"]
EXP = create_account(entity, "5000", "Operating Expense", "expense")["id"]

# 1) Managed service / support company.
support_wf = workflow("support", "support_service.json")
support = create_case(
    "support", support_wf["id"], "Managed service customer incident",
    budget=5000.0,
    metadata={"classification": "confidential", "customer": "CUSTOMER-DEMO", "sla": "P2-4H"},
)
add_participants(support["id"])
support_run = wait_case(support["id"])
assert support_run["case"]["state"] == "completed"
create_budget(support["id"], 500_000)
invoice(support["id"], "receivable", "CUSTOMER-DEMO", 300_000, f"SUPPORT-AR-{STAMP}")
ap_support = invoice(support["id"], "payable", "CLOUD-PROVIDER-DEMO", 80_000, f"SUPPORT-AP-{STAMP}")
api("POST", f"/v1/company/economics/invoices/{ap_support['id']}/approve", {"approved_by_principal_id": OWNER})
journal(support["id"], "Recognize managed-service revenue", [
    {"account_id": AR, "debit_minor": 300_000, "memo": "customer receivable"},
    {"account_id": REV, "credit_minor": 300_000, "memo": "service revenue"},
], "support-revenue")
journal(support["id"], "Recognize support delivery cost", [
    {"account_id": EXP, "debit_minor": 80_000, "memo": "cloud/support cost"},
    {"account_id": AP, "credit_minor": 80_000, "memo": "provider payable"},
], "support-cost")
support_econ = economics(support["id"])
assert support_econ["margin_minor"] == 220_000

# 2) Procurement / trading company.
proc_wf = workflow("procurement", "procurement.json")
proc = create_case(
    "procurement", proc_wf["id"], "Governed equipment procurement",
    budget=50000.0,
    metadata={"classification": "confidential", "department": "operations", "need": "equipment"},
)
add_participants(proc["id"])
proc_run = wait_case(proc["id"])
assert proc_run["case"]["state"] == "completed"
create_budget(proc["id"], 5_000_000)
commit = api("POST", "/v1/company/economics/commitments", {
    "case_id": proc["id"], "description": "Approved supplier package",
    "counterparty_ref": "SUPPLIER-DEMO", "amount_minor": 1_200_000,
    "currency": "EUR", "requested_by_principal_id": PLANNER,
})
api("POST", f"/v1/company/economics/commitments/{commit['id']}/approve", {"approved_by_principal_id": OWNER})
ap_proc = invoice(proc["id"], "payable", "SUPPLIER-DEMO", 1_180_000, f"PROC-AP-{STAMP}")
api("POST", f"/v1/company/economics/invoices/{ap_proc['id']}/approve", {"approved_by_principal_id": OWNER})
payment = api("POST", "/v1/company/economics/payment-intents", {
    "case_id": proc["id"], "invoice_id": ap_proc["id"], "direction": "outbound",
    "amount_minor": 1_180_000, "currency": "EUR",
    "idempotency_key": f"alp14-{STAMP}-proc-payment",
    "requested_by_principal_id": PLANNER,
    "metadata": {"demo": "procurement", "external_execution": False},
})
authorized = api("POST", f"/v1/company/economics/payment-intents/{payment['id']}/authorize", {
    "authorized_by_principal_id": OWNER,
})
assert authorized["executed_at"] is None
journal(proc["id"], "Recognize procurement expense", [
    {"account_id": EXP, "debit_minor": 1_180_000, "memo": "procurement expense"},
    {"account_id": AP, "credit_minor": 1_180_000, "memo": "supplier payable"},
], "proc-expense")
proc_econ = economics(proc["id"])
assert proc_econ["ledger_balanced"] is True
assert proc_econ["authorized_outbound_minor"] == 1_180_000
assert proc_econ["payment_execution"] == "external-only"

# 3) Enterprise transition across vendor systems, with shadow replay.
trans_wf = workflow("migration", "enterprise_transition.json")
transition = create_case(
    "migration", trans_wf["id"], "SAP + Salesforce transition rehearsal",
    metadata={"classification": "restricted", "source": "legacy-enterprise", "target": "new-enterprise"},
)
add_participants(transition["id"])
transition_run = wait_case(transition["id"])
assert transition_run["case"]["state"] == "completed"
blueprint = api("POST", "/v1/company/blueprints", {
    "name": f"alp14-transition-{STAMP}", "version": 1,
    "source_workflow_definition_id": trans_wf["id"],
    "created_by_principal_id": OWNER,
    "classification": "confidential",
    "business_context": {"scenario": "cross-vendor transition", "systems": ["sap", "salesforce"]},
    "sensitive": {"customer_names": ["REDACT-ME"], "internal_notes": "confidential demo material"},
})
for mapping in [
    {"domain":"sales","canonical_type":"BusinessParty","target_system":"salesforce","target_type":"Account","owner_system":"salesforce","direction":"bidirectional","mapping":{"external_id":"Account.Id"}},
    {"domain":"finance","canonical_type":"BusinessParty","target_system":"sap","target_type":"BusinessPartner","owner_system":"sap","direction":"bidirectional","mapping":{"external_id":"BusinessPartner"}},
]:
    mapping["created_by_principal_id"] = OWNER
    api("POST", f"/v1/company/blueprints/{blueprint['id']}/mappings", mapping)
connector = api("POST", "/v1/company/connectors", {
    "name": f"mulesoft-shadow-{STAMP}", "provider": "mulesoft", "transport": "api",
    "environment": "shadow", "mode": "read_only", "credential_ref": "vault://alp14/demo/mulesoft",
    "capabilities": ["salesforce.read", "sap.read"],
    "config": {"purpose": "transport-only", "business_logic_owner": "alp"},
    "created_by_principal_id": OWNER,
})
assert connector["mode"] == "read_only"
shadow = api("POST", "/v1/company/simulations/replay", {
    "source_case_id": transition["id"], "target_blueprint_id": blueprint["id"],
    "requested_by_principal_id": OWNER, "mode": "shadow",
})
assert shadow["result"]["process_equivalence"] == 100.0
assert shadow["result"]["side_effects_executed"] is False
redacted = api("GET", f"/v1/company/blueprints/{blueprint['id']}/export/redacted")
redacted_text = json.dumps(redacted)
assert "REDACT-ME" not in redacted_text

# 4) Project delivery company.
project_wf = workflow("project_demo", "project_delivery.json")
now = time.time()
start = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(now))
end = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(now + 30 * 86400))
project = create_case(
    "project_demo", project_wf["id"], "Client implementation project",
    budget=120000.0, start=start, end=end,
    metadata={"classification": "confidential", "client": "CLIENT-DEMO", "project_code": f"PRJ-{STAMP}"},
)
add_participants(project["id"])
project_run = wait_case(project["id"])
assert project_run["case"]["state"] == "completed"
assert project_run["case"]["planned_start_at"] is not None
assert project_run["case"]["planned_end_at"] is not None
create_budget(project["id"], 12_000_000)
invoice(project["id"], "receivable", "CLIENT-DEMO", 15_000_000, f"PROJECT-AR-{STAMP}")
ap_project = invoice(project["id"], "payable", "SUBCONTRACTOR-DEMO", 9_000_000, f"PROJECT-AP-{STAMP}")
api("POST", f"/v1/company/economics/invoices/{ap_project['id']}/approve", {"approved_by_principal_id": OWNER})
journal(project["id"], "Recognize project revenue", [
    {"account_id": AR, "debit_minor": 15_000_000, "memo": "client receivable"},
    {"account_id": REV, "credit_minor": 15_000_000, "memo": "project revenue"},
], "project-revenue")
journal(project["id"], "Recognize project delivery cost", [
    {"account_id": EXP, "debit_minor": 9_000_000, "memo": "delivery cost"},
    {"account_id": AP, "credit_minor": 9_000_000, "memo": "subcontractor payable"},
], "project-cost")
project_econ = economics(project["id"])
assert project_econ["margin_minor"] == 6_000_000

performance = api("GET", "/v1/company/workplace/performance")
roles = {row["role"] for row in performance}
assert {"planner", "researcher", "worker", "verifier"}.issubset(roles)

summary = {
    "support_service": {
        "case_id": support["id"], "state": support_run["case"]["state"],
        "revenue_minor": support_econ["revenue_minor"], "expense_minor": support_econ["expense_minor"],
        "margin_minor": support_econ["margin_minor"],
    },
    "procurement": {
        "case_id": proc["id"], "state": proc_run["case"]["state"],
        "commitments_minor": proc_econ["commitments_minor"],
        "authorized_outbound_minor": proc_econ["authorized_outbound_minor"],
        "payment_execution": proc_econ["payment_execution"],
    },
    "enterprise_transition": {
        "case_id": transition["id"], "state": transition_run["case"]["state"],
        "blueprint_id": blueprint["id"], "process_equivalence": shadow["result"]["process_equivalence"],
        "side_effects_executed": shadow["result"]["side_effects_executed"],
    },
    "project_delivery": {
        "case_id": project["id"], "state": project_run["case"]["state"],
        "budget_minor": project_econ["budget_minor"], "revenue_minor": project_econ["revenue_minor"],
        "expense_minor": project_econ["expense_minor"], "margin_minor": project_econ["margin_minor"],
        "participants": len(project_run["participants"]),
    },
    "workplace_agents": len(performance),
}
print(json.dumps(summary, indent=2, sort_keys=True))
print("SUPPORT_SERVICE_DEMO_ASSERTION=PASS")
print("PROCUREMENT_ECONOMICS_DEMO_ASSERTION=PASS")
print("ENTERPRISE_TRANSITION_DEMO_ASSERTION=PASS")
print("PROJECT_DELIVERY_DEMO_ASSERTION=PASS")
print("REFERENCE_COMPANY_GENERALITY_ASSERTION=PASS")
