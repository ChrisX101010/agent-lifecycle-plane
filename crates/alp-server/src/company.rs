use crate::{execution, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
use uuid::Uuid;

const COMPANY_LEASE_SECONDS: i64 = 60;

#[derive(Debug, Deserialize)]
struct CreateWorkflow {
    name: String,
    version: i32,
    case_type: String,
    created_by_principal_id: Uuid,
    definition: Value,
}

#[derive(Debug, Deserialize, Clone)]
struct WorkflowDefinition {
    nodes: Vec<WorkflowNode>,
}

#[derive(Debug, Deserialize, Clone)]
struct WorkflowNode {
    key: String,
    #[serde(default = "default_task_kind")]
    kind: String,
    role: String,
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    condition: Option<Value>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    checklist: Vec<String>,
    #[serde(default)]
    max_attempts: Option<i32>,
    #[serde(default)]
    sla_seconds: Option<i64>,
    #[serde(default)]
    delay_seconds: Option<i64>,
    #[serde(default)]
    wait_for: Option<String>,
}

fn default_task_kind() -> String {
    "task".into()
}

#[derive(Debug, Deserialize)]
struct CreateCase {
    case_type: String,
    title: String,
    #[serde(default)]
    description: String,
    workflow_definition_id: Uuid,
    created_by_principal_id: Uuid,
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    priority: Option<i32>,
    #[serde(default)]
    budget_amount: Option<f64>,
    #[serde(default)]
    budget_currency: Option<String>,
    #[serde(default)]
    planned_start_at: Option<DateTime<Utc>>,
    #[serde(default)]
    planned_end_at: Option<DateTime<Utc>>,
    #[serde(default)]
    metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct AddParticipant {
    principal_id: Uuid,
    assignment_role: String,
    #[serde(default)]
    allocation_percent: Option<f64>,
    authorized_by_principal_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct CompleteWorkItem {
    principal_id: Uuid,
    status: String,
    #[serde(default)]
    output: Value,
    #[serde(default)]
    checklist_completed: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct HeartbeatWorkItem {
    principal_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct AppendCaseEvent {
    principal_id: Uuid,
    kind: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    payload: Value,
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/company/workflows",
            post(create_workflow).get(list_workflows),
        )
        .route("/v1/company/workflows/{id}", get(get_workflow))
        .route("/v1/company/cases", post(create_case).get(list_cases))
        .route("/v1/company/cases/{id}", get(get_case))
        .route("/v1/company/cases/{id}/participants", post(add_participant))
        .route("/v1/company/cases/{id}/events", post(append_case_event))
        .route("/v1/company/agents/{id}/claim", post(claim_work_item))
        .route(
            "/v1/company/work-items/{id}/heartbeat",
            post(heartbeat_work_item),
        )
        .route(
            "/v1/company/work-items/{id}/complete",
            post(complete_work_item),
        )
        .route(
            "/v1/company/workplace/performance",
            get(workplace_performance),
        )
}

pub(crate) fn spawn_reconciler(db: PgPool) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = reconcile_once(&db).await {
                tracing::error!(error = ?error, "ALP company reconciliation pass failed");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

async fn create_workflow(
    State(state): State<AppState>,
    Json(input): Json<CreateWorkflow>,
) -> ApiResult {
    execution::ensure_principal_kind(&state.db, input.created_by_principal_id, "user").await?;
    execution::ensure_capability(&state.db, input.created_by_principal_id, "task.approve").await?;

    if input.name.trim().is_empty() || input.case_type.trim().is_empty() || input.version < 1 {
        return Err((
            StatusCode::BAD_REQUEST,
            "workflow name/case_type must be non-empty and version >= 1".into(),
        ));
    }

    let definition: WorkflowDefinition =
        serde_json::from_value(input.definition.clone()).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid workflow definition: {e}"),
            )
        })?;
    validate_definition(&definition)?;

    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO workflow_definitions
           (id,name,version,case_type,state,definition_json,created_by_principal_id,created_at)
           VALUES ($1,$2,$3,$4,'published',$5,$6,now())"#,
    )
    .bind(id)
    .bind(input.name.trim())
    .bind(input.version)
    .bind(input.case_type.trim())
    .bind(input.definition.to_string())
    .bind(input.created_by_principal_id)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(serde_json::json!({
        "id": id,
        "name": input.name.trim(),
        "version": input.version,
        "case_type": input.case_type.trim(),
        "state": "published"
    })))
}

async fn list_workflows(State(state): State<AppState>) -> ApiResult {
    let rows = sqlx::query(
        "SELECT id,name,version,case_type,state,created_by_principal_id,created_at FROM workflow_definitions ORDER BY name,version DESC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(Value::Array(
        rows.into_iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.get::<Uuid,_>("id"),
                    "name": r.get::<String,_>("name"),
                    "version": r.get::<i32,_>("version"),
                    "case_type": r.get::<String,_>("case_type"),
                    "state": r.get::<String,_>("state"),
                    "created_by_principal_id": r.get::<Uuid,_>("created_by_principal_id"),
                    "created_at": r.get::<DateTime<Utc>,_>("created_at")
                })
            })
            .collect(),
    )))
}

async fn get_workflow(State(state): State<AppState>, Path(id): Path<Uuid>) -> ApiResult {
    let row=sqlx::query("SELECT id,name,version,case_type,state,definition_json,created_by_principal_id,created_at FROM workflow_definitions WHERE id=$1")
        .bind(id).fetch_optional(&state.db).await.map_err(internal)?.ok_or((StatusCode::NOT_FOUND,"workflow definition not found".into()))?;
    Ok(Json(serde_json::json!({
        "id":row.get::<Uuid,_>("id"),"name":row.get::<String,_>("name"),"version":row.get::<i32,_>("version"),
        "case_type":row.get::<String,_>("case_type"),"state":row.get::<String,_>("state"),
        "definition":parse_json(&row.get::<String,_>("definition_json")),
        "created_by_principal_id":row.get::<Uuid,_>("created_by_principal_id"),"created_at":row.get::<DateTime<Utc>,_>("created_at")
    })))
}

async fn create_case(
    State(state): State<AppState>,
    Json(input): Json<CreateCase>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    execution::ensure_principal_kind(&state.db, input.created_by_principal_id, "user").await?;
    execution::ensure_capability(&state.db, input.created_by_principal_id, "task.approve").await?;

    if input.title.trim().is_empty() || input.case_type.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "case title and case_type cannot be empty".into(),
        ));
    }
    if input.budget_amount.is_some() != input.budget_currency.is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            "budget_amount and budget_currency must be supplied together".into(),
        ));
    }
    if let (Some(start), Some(end)) = (
        input.planned_start_at.as_ref(),
        input.planned_end_at.as_ref(),
    ) {
        if end <= start {
            return Err((
                StatusCode::BAD_REQUEST,
                "planned_end_at must be after planned_start_at".into(),
            ));
        }
    }

    let workflow = sqlx::query(
        "SELECT name,version,case_type,state,definition_json FROM workflow_definitions WHERE id=$1",
    )
    .bind(input.workflow_definition_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or((
        StatusCode::NOT_FOUND,
        "workflow definition not found".into(),
    ))?;

    if workflow.get::<String, _>("state") != "published" {
        return Err((StatusCode::CONFLICT, "workflow is not published".into()));
    }
    if workflow.get::<String, _>("case_type") != input.case_type.trim() {
        return Err((
            StatusCode::BAD_REQUEST,
            "case_type does not match workflow definition".into(),
        ));
    }

    let definition_text = workflow.get::<String, _>("definition_json");
    let definition: WorkflowDefinition = serde_json::from_str(&definition_text).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("stored workflow is invalid: {e}"),
        )
    })?;
    validate_definition(&definition)?;

    let environment = input.environment.unwrap_or_else(|| "sandbox".into());
    if !matches!(environment.as_str(), "sandbox" | "shadow" | "production") {
        return Err((
            StatusCode::BAD_REQUEST,
            "environment must be sandbox, shadow, or production".into(),
        ));
    }
    let priority = input.priority.unwrap_or(50).clamp(0, 100);
    let id = Uuid::new_v4();
    let metadata = input.metadata.unwrap_or_else(|| serde_json::json!({}));
    let mut tx = state.db.begin().await.map_err(internal)?;

    sqlx::query(
        r#"INSERT INTO company_cases
           (id,case_type,title,description,workflow_definition_id,workflow_version,environment,state,
            priority,budget_amount,budget_currency,planned_start_at,planned_end_at,created_by_principal_id,
            metadata_json,created_at,updated_at)
           VALUES ($1,$2,$3,$4,$5,$6,$7,'running',$8,$9,$10,$11,$12,$13,$14,now(),now())"#,
    )
    .bind(id)
    .bind(input.case_type.trim())
    .bind(input.title.trim())
    .bind(input.description)
    .bind(input.workflow_definition_id)
    .bind(workflow.get::<i32,_>("version"))
    .bind(&environment)
    .bind(priority)
    .bind(input.budget_amount)
    .bind(&input.budget_currency)
    .bind(input.planned_start_at)
    .bind(input.planned_end_at)
    .bind(input.created_by_principal_id)
    .bind(metadata.to_string())
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    for node in &definition.nodes {
        let mut node_input = node.input.clone().unwrap_or_else(|| serde_json::json!({}));
        if !node_input.is_object() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("node {} input must be a JSON object", node.key),
            ));
        }
        if let Some(obj) = node_input.as_object_mut() {
            if let Some(sla) = node.sla_seconds {
                obj.insert("_sla_seconds".into(), Value::from(sla.max(0)));
            }
            if let Some(delay) = node.delay_seconds {
                obj.insert("_delay_seconds".into(), Value::from(delay.max(0)));
            }
            if let Some(wait_for) = &node.wait_for {
                obj.insert("_wait_for".into(), Value::from(wait_for.clone()));
            }
        }

        sqlx::query(
            r#"INSERT INTO case_work_items
               (id,case_id,node_key,kind,role,state,required,attempt,max_attempts,depends_on_json,
                condition_json,input_json,checklist_json,created_at)
               VALUES ($1,$2,$3,$4,$5,'blocked',$6,0,$7,$8,$9,$10,$11,now())"#,
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(&node.key)
        .bind(&node.kind)
        .bind(&node.role)
        .bind(node.required.unwrap_or(true))
        .bind(node.max_attempts.unwrap_or(3).clamp(1, 10))
        .bind(serde_json::to_string(&node.depends_on).unwrap_or_else(|_| "[]".into()))
        .bind(node.condition.as_ref().map(Value::to_string))
        .bind(node_input.to_string())
        .bind(serde_json::to_string(&node.checklist).unwrap_or_else(|_| "[]".into()))
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    }

    append_case_event_tx(
        &mut tx,
        id,
        None,
        Some(input.created_by_principal_id),
        "case.created",
        "business case created from immutable workflow version",
        &serde_json::json!({"workflow_version": workflow.get::<i32,_>("version"), "environment": environment}),
    ).await?;

    advance_case_tx(&mut tx, id).await?;
    tx.commit().await.map_err(internal)?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": id,
            "case_type": input.case_type.trim(),
            "title": input.title.trim(),
            "workflow_definition_id": input.workflow_definition_id,
            "workflow_version": workflow.get::<i32,_>("version"),
            "environment": environment,
            "state": "running",
            "priority": priority,
            "budget_amount": input.budget_amount,
            "budget_currency": input.budget_currency,
            "planned_start_at": input.planned_start_at,
            "planned_end_at": input.planned_end_at
        })),
    ))
}

async fn list_cases(State(state): State<AppState>) -> ApiResult {
    let rows = sqlx::query(
        r#"SELECT id,case_type,title,state,workflow_definition_id,workflow_version,environment,priority,
                  budget_amount,budget_currency,planned_start_at,planned_end_at,actual_start_at,actual_end_at,
                  created_by_principal_id,created_at,updated_at
           FROM company_cases ORDER BY created_at DESC LIMIT 200"#,
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(Value::Array(
        rows.into_iter().map(case_summary_json).collect(),
    )))
}

async fn get_case(State(state): State<AppState>, Path(id): Path<Uuid>) -> ApiResult {
    let case = sqlx::query(
        r#"SELECT id,case_type,title,description,state,workflow_definition_id,workflow_version,environment,priority,
                  budget_amount,budget_currency,planned_start_at,planned_end_at,actual_start_at,actual_end_at,
                  created_by_principal_id,metadata_json,created_at,updated_at
           FROM company_cases WHERE id=$1"#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "case not found".into()))?;

    let items = sqlx::query(
        r#"SELECT id,case_id,node_key,kind,role,state,required,attempt,max_attempts,depends_on_json,
                  condition_json,input_json,output_json,checklist_json,assigned_principal_id,claimed_at,
                  lease_expires_at,available_at,due_at,completed_at,created_at
           FROM case_work_items WHERE case_id=$1 ORDER BY created_at,node_key"#,
    )
    .bind(id).fetch_all(&state.db).await.map_err(internal)?;

    let participants = sqlx::query(
        r#"SELECT cp.principal_id,p.name,cp.assignment_role,cp.allocation_percent,cp.assigned_at
           FROM case_participants cp JOIN principals p ON p.id=cp.principal_id
           WHERE cp.case_id=$1 ORDER BY cp.assigned_at"#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    let reviews = sqlx::query(
        r#"SELECT id,work_item_id,reviewer_principal_id,score,decision,dimensions_json,notes,created_at
           FROM case_reviews WHERE case_id=$1 ORDER BY created_at"#,
    )
    .bind(id).fetch_all(&state.db).await.map_err(internal)?;

    let events = sqlx::query(
        r#"SELECT id,work_item_id,principal_id,kind,message,payload_json,created_at
           FROM case_events WHERE case_id=$1 ORDER BY created_at"#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(serde_json::json!({
        "case": {
            "id": case.get::<Uuid,_>("id"),
            "case_type": case.get::<String,_>("case_type"),
            "title": case.get::<String,_>("title"),
            "description": case.get::<String,_>("description"),
            "state": case.get::<String,_>("state"),
            "workflow_definition_id": case.get::<Uuid,_>("workflow_definition_id"),
            "workflow_version": case.get::<i32,_>("workflow_version"),
            "environment": case.get::<String,_>("environment"),
            "priority": case.get::<i32,_>("priority"),
            "budget_amount": case.get::<Option<f64>,_>("budget_amount"),
            "budget_currency": case.get::<Option<String>,_>("budget_currency"),
            "planned_start_at": case.get::<Option<DateTime<Utc>>,_>("planned_start_at"),
            "planned_end_at": case.get::<Option<DateTime<Utc>>,_>("planned_end_at"),
            "actual_start_at": case.get::<DateTime<Utc>,_>("actual_start_at"),
            "actual_end_at": case.get::<Option<DateTime<Utc>>,_>("actual_end_at"),
            "created_by_principal_id": case.get::<Uuid,_>("created_by_principal_id"),
            "metadata": parse_json(&case.get::<String,_>("metadata_json")),
            "created_at": case.get::<DateTime<Utc>,_>("created_at"),
            "updated_at": case.get::<DateTime<Utc>,_>("updated_at")
        },
        "work_items": items.into_iter().map(work_item_json).collect::<Vec<_>>(),
        "participants": participants.into_iter().map(|r| serde_json::json!({
            "principal_id": r.get::<Uuid,_>("principal_id"),
            "name": r.get::<String,_>("name"),
            "assignment_role": r.get::<String,_>("assignment_role"),
            "allocation_percent": r.get::<f64,_>("allocation_percent"),
            "assigned_at": r.get::<DateTime<Utc>,_>("assigned_at")
        })).collect::<Vec<_>>(),
        "reviews": reviews.into_iter().map(|r| serde_json::json!({
            "id": r.get::<Uuid,_>("id"),
            "work_item_id": r.get::<Option<Uuid>,_>("work_item_id"),
            "reviewer_principal_id": r.get::<Uuid,_>("reviewer_principal_id"),
            "score": r.get::<f64,_>("score"),
            "decision": r.get::<String,_>("decision"),
            "dimensions": parse_json(&r.get::<String,_>("dimensions_json")),
            "notes": r.get::<String,_>("notes"),
            "created_at": r.get::<DateTime<Utc>,_>("created_at")
        })).collect::<Vec<_>>(),
        "events": events.into_iter().map(|r| serde_json::json!({
            "id": r.get::<Uuid,_>("id"),
            "work_item_id": r.get::<Option<Uuid>,_>("work_item_id"),
            "principal_id": r.get::<Option<Uuid>,_>("principal_id"),
            "kind": r.get::<String,_>("kind"),
            "message": r.get::<String,_>("message"),
            "payload": parse_json(&r.get::<String,_>("payload_json")),
            "created_at": r.get::<DateTime<Utc>,_>("created_at")
        })).collect::<Vec<_>>()
    })))
}

async fn add_participant(
    State(state): State<AppState>,
    Path(case_id): Path<Uuid>,
    Json(input): Json<AddParticipant>,
) -> ApiResult {
    execution::ensure_principal_kind(&state.db, input.authorized_by_principal_id, "user").await?;
    execution::ensure_capability(&state.db, input.authorized_by_principal_id, "task.approve")
        .await?;
    ensure_active_principal(&state.db, input.principal_id).await?;

    let allocation = input.allocation_percent.unwrap_or(100.0);
    if !(0.0..=100.0).contains(&allocation) || input.assignment_role.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "allocation_percent must be 0..100 and assignment_role non-empty".into(),
        ));
    }

    let result = sqlx::query(
        r#"INSERT INTO case_participants(case_id,principal_id,assignment_role,allocation_percent,assigned_at)
           VALUES($1,$2,$3,$4,now())
           ON CONFLICT(case_id,principal_id,assignment_role)
           DO UPDATE SET allocation_percent=EXCLUDED.allocation_percent,assigned_at=now()"#,
    )
    .bind(case_id).bind(input.principal_id).bind(input.assignment_role.trim()).bind(allocation)
    .execute(&state.db).await.map_err(internal)?;

    if result.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, "case not found".into()));
    }

    Ok(Json(serde_json::json!({
        "case_id": case_id,
        "principal_id": input.principal_id,
        "assignment_role": input.assignment_role.trim(),
        "allocation_percent": allocation
    })))
}

async fn append_case_event(
    State(state): State<AppState>,
    Path(case_id): Path<Uuid>,
    Json(input): Json<AppendCaseEvent>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    ensure_active_principal(&state.db, input.principal_id).await?;
    if input.kind.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "event kind cannot be empty".into()));
    }
    let mut tx = state.db.begin().await.map_err(internal)?;
    ensure_case_exists_tx(&mut tx, case_id).await?;
    append_case_event_tx(
        &mut tx,
        case_id,
        None,
        Some(input.principal_id),
        input.kind.trim(),
        if input.message.trim().is_empty() {
            "external business event received"
        } else {
            input.message.trim()
        },
        &input.payload,
    )
    .await?;
    advance_case_tx(&mut tx, case_id).await?;
    tx.commit().await.map_err(internal)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"case_id":case_id,"kind":input.kind,"state":"recorded"})),
    ))
}

async fn claim_work_item(
    State(state): State<AppState>,
    Path(principal_id): Path<Uuid>,
) -> ApiResult {
    let role = active_agent_role(&state.db, principal_id).await?;
    execution::ensure_capability(&state.db, principal_id, "case.work").await?;

    let mut tx = state.db.begin().await.map_err(internal)?;
    let existing = sqlx::query(
        r#"SELECT wi.id,wi.case_id,wi.node_key,wi.kind,wi.role,wi.state,wi.required,wi.attempt,wi.max_attempts,
                  wi.depends_on_json,wi.condition_json,wi.input_json,wi.output_json,wi.checklist_json,
                  wi.assigned_principal_id,wi.claimed_at,wi.lease_expires_at,wi.available_at,wi.due_at,
                  wi.completed_at,wi.created_at
           FROM case_work_items wi JOIN company_cases c ON c.id=wi.case_id
           WHERE c.state='running' AND wi.state='running' AND wi.assigned_principal_id=$1
             AND wi.lease_expires_at > now()
           ORDER BY wi.claimed_at LIMIT 1 FOR UPDATE OF wi SKIP LOCKED"#,
    )
    .bind(principal_id).fetch_optional(&mut *tx).await.map_err(internal)?;

    if let Some(row) = existing {
        let kind = row.get::<String, _>("kind");
        if kind == "supervisor" {
            ensure_capability_tx(&mut tx, principal_id, "case.supervise").await?;
        }
        tx.commit().await.map_err(internal)?;
        return Ok(Json(
            serde_json::json!({"state":"claimed","resumed":true,"work_item":work_item_json(row)}),
        ));
    }

    let candidate = sqlx::query(
        r#"SELECT wi.id,wi.kind
           FROM case_work_items wi JOIN company_cases c ON c.id=wi.case_id
           WHERE c.state='running' AND wi.state='ready' AND wi.role=$1
             AND (wi.available_at IS NULL OR wi.available_at <= now())
           ORDER BY c.priority DESC,c.created_at,wi.created_at
           LIMIT 1 FOR UPDATE OF wi SKIP LOCKED"#,
    )
    .bind(&role)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal)?;

    let Some(candidate) = candidate else {
        tx.commit().await.map_err(internal)?;
        return Ok(Json(serde_json::json!({"state":"no_work"})));
    };
    if candidate.get::<String, _>("kind") == "supervisor" {
        ensure_capability_tx(&mut tx, principal_id, "case.supervise").await?;
    }

    let id = candidate.get::<Uuid, _>("id");
    let row = sqlx::query(
        r#"UPDATE case_work_items
           SET state='running',assigned_principal_id=$1,attempt=attempt+1,claimed_at=now(),
               lease_expires_at=now()+($2::bigint * interval '1 second')
           WHERE id=$3 AND state='ready'
           RETURNING id,case_id,node_key,kind,role,state,required,attempt,max_attempts,depends_on_json,
                     condition_json,input_json,output_json,checklist_json,assigned_principal_id,claimed_at,
                     lease_expires_at,available_at,due_at,completed_at,created_at"#,
    )
    .bind(principal_id).bind(COMPANY_LEASE_SECONDS).bind(id)
    .fetch_one(&mut *tx).await.map_err(internal)?;

    append_case_event_tx(
        &mut tx,
        row.get::<Uuid, _>("case_id"),
        Some(id),
        Some(principal_id),
        "work.claimed",
        "agent claimed governed company work item",
        &serde_json::json!({"role": role}),
    )
    .await?;
    tx.commit().await.map_err(internal)?;

    Ok(Json(
        serde_json::json!({"state":"claimed","resumed":false,"work_item":work_item_json(row)}),
    ))
}

async fn heartbeat_work_item(
    State(state): State<AppState>,
    Path(work_item_id): Path<Uuid>,
    Json(input): Json<HeartbeatWorkItem>,
) -> ApiResult {
    let result = sqlx::query(
        r#"UPDATE case_work_items SET lease_expires_at=now()+($1::bigint * interval '1 second')
           WHERE id=$2 AND state='running' AND assigned_principal_id=$3 AND lease_expires_at > now()
           RETURNING case_id,lease_expires_at"#,
    )
    .bind(COMPANY_LEASE_SECONDS)
    .bind(work_item_id)
    .bind(input.principal_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or((
        StatusCode::CONFLICT,
        "work item has no renewable active lease for this principal".into(),
    ))?;

    Ok(Json(serde_json::json!({
        "work_item_id": work_item_id,
        "case_id": result.get::<Uuid,_>("case_id"),
        "state": "renewed",
        "lease_expires_at": result.get::<DateTime<Utc>,_>("lease_expires_at")
    })))
}

async fn complete_work_item(
    State(state): State<AppState>,
    Path(work_item_id): Path<Uuid>,
    Json(input): Json<CompleteWorkItem>,
) -> ApiResult {
    if !matches!(input.status.as_str(), "succeeded" | "failed") {
        return Err((
            StatusCode::BAD_REQUEST,
            "status must be succeeded or failed".into(),
        ));
    }

    let mut tx = state.db.begin().await.map_err(internal)?;
    let row = sqlx::query(
        r#"SELECT id,case_id,node_key,kind,role,state,required,attempt,max_attempts,depends_on_json,
                  condition_json,input_json,output_json,checklist_json,assigned_principal_id,claimed_at,
                  lease_expires_at,available_at,due_at,completed_at,created_at
           FROM case_work_items WHERE id=$1 FOR UPDATE"#,
    )
    .bind(work_item_id).fetch_optional(&mut *tx).await.map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "work item not found".into()))?;

    let state_now = row.get::<String, _>("state");
    if matches!(state_now.as_str(), "completed" | "failed" | "skipped") {
        tx.commit().await.map_err(internal)?;
        return Ok(Json(
            serde_json::json!({"work_item_id":work_item_id,"state":state_now,"idempotent":true}),
        ));
    }
    if state_now != "running"
        || row.get::<Option<Uuid>, _>("assigned_principal_id") != Some(input.principal_id)
    {
        return Err((
            StatusCode::FORBIDDEN,
            "work item is not actively assigned to this principal".into(),
        ));
    }
    if row
        .get::<Option<DateTime<Utc>>, _>("lease_expires_at")
        .is_some_and(|t| t <= Utc::now())
    {
        return Err((StatusCode::CONFLICT, "work item lease expired".into()));
    }

    let role = row.get::<String, _>("role");
    ensure_role_tx(&mut tx, input.principal_id, &role).await?;
    ensure_capability_tx(&mut tx, input.principal_id, "case.work").await?;
    let kind = row.get::<String, _>("kind");
    if kind == "supervisor" {
        ensure_capability_tx(&mut tx, input.principal_id, "case.supervise").await?;
    }

    let expected: Vec<String> =
        serde_json::from_str(&row.get::<String, _>("checklist_json")).unwrap_or_default();
    if input.status == "succeeded" {
        let done: HashSet<_> = input.checklist_completed.iter().collect();
        let missing: Vec<_> = expected
            .iter()
            .filter(|x| !done.contains(x))
            .cloned()
            .collect();
        if !missing.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "required checklist items are incomplete: {}",
                    missing.join(", ")
                ),
            ));
        }
    }

    let case_id = row.get::<Uuid, _>("case_id");
    let attempt = row.get::<i32, _>("attempt");
    let max_attempts = row.get::<i32, _>("max_attempts");
    let claimed_at = row.get::<Option<DateTime<Utc>>, _>("claimed_at");
    let due_at = row.get::<Option<DateTime<Utc>>, _>("due_at");
    let duration_ms = claimed_at.map(|t| (Utc::now() - t).num_milliseconds().max(0));
    let sla_met = due_at.map(|t| Utc::now() <= t);
    let quality_score = input
        .output
        .get("quality_score")
        .and_then(Value::as_f64)
        .or_else(|| input.output.get("score").and_then(Value::as_f64));

    if input.status == "failed" && attempt < max_attempts {
        sqlx::query(
            r#"UPDATE case_work_items SET state='ready',assigned_principal_id=NULL,output_json=$1,
               claimed_at=NULL,lease_expires_at=NULL WHERE id=$2"#,
        )
        .bind(input.output.to_string())
        .bind(work_item_id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        record_performance_tx(
            &mut tx,
            input.principal_id,
            case_id,
            work_item_id,
            &role,
            "retry",
            duration_ms,
            attempt,
            quality_score,
            sla_met,
        )
        .await?;
        append_case_event_tx(
            &mut tx,
            case_id,
            Some(work_item_id),
            Some(input.principal_id),
            "work.retry",
            "agent reported failure; bounded retry scheduled",
            &input.output,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        return Ok(Json(
            serde_json::json!({"work_item_id":work_item_id,"state":"ready","retry":true,"attempt":attempt,"max_attempts":max_attempts}),
        ));
    }

    if input.status == "failed" {
        sqlx::query("UPDATE case_work_items SET state='failed',output_json=$1,completed_at=now(),lease_expires_at=NULL WHERE id=$2")
            .bind(input.output.to_string()).bind(work_item_id).execute(&mut *tx).await.map_err(internal)?;
        sqlx::query("UPDATE company_cases SET state='needs_attention',updated_at=now() WHERE id=$1 AND state='running'")
            .bind(case_id).execute(&mut *tx).await.map_err(internal)?;
        record_performance_tx(
            &mut tx,
            input.principal_id,
            case_id,
            work_item_id,
            &role,
            "failed",
            duration_ms,
            attempt,
            quality_score,
            sla_met,
        )
        .await?;
        append_case_event_tx(
            &mut tx,
            case_id,
            Some(work_item_id),
            Some(input.principal_id),
            "case.needs_attention",
            "work item exhausted bounded retries",
            &input.output,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        return Ok(Json(
            serde_json::json!({"work_item_id":work_item_id,"state":"failed","case_state":"needs_attention"}),
        ));
    }

    sqlx::query("UPDATE case_work_items SET state='completed',output_json=$1,completed_at=now(),lease_expires_at=NULL WHERE id=$2")
        .bind(input.output.to_string()).bind(work_item_id).execute(&mut *tx).await.map_err(internal)?;
    record_performance_tx(
        &mut tx,
        input.principal_id,
        case_id,
        work_item_id,
        &role,
        "succeeded",
        duration_ms,
        attempt,
        quality_score,
        sla_met,
    )
    .await?;
    append_case_event_tx(
        &mut tx,
        case_id,
        Some(work_item_id),
        Some(input.principal_id),
        "work.completed",
        "agent completed governed company work item",
        &input.output,
    )
    .await?;

    if kind == "supervisor" {
        let score = input
            .output
            .get("score")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .clamp(0.0, 100.0);
        let decision = input
            .output
            .get("decision")
            .and_then(Value::as_str)
            .unwrap_or("review");
        let dimensions = input
            .output
            .get("dimensions")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let notes = input
            .output
            .get("notes")
            .and_then(Value::as_str)
            .unwrap_or("");
        sqlx::query(
            r#"INSERT INTO case_reviews(id,case_id,work_item_id,reviewer_principal_id,score,decision,dimensions_json,notes,created_at)
               VALUES($1,$2,$3,$4,$5,$6,$7,$8,now())"#,
        )
        .bind(Uuid::new_v4()).bind(case_id).bind(work_item_id).bind(input.principal_id)
        .bind(score).bind(decision).bind(dimensions.to_string()).bind(notes)
        .execute(&mut *tx).await.map_err(internal)?;
        if decision != "pass" {
            sqlx::query("UPDATE company_cases SET state='needs_attention',updated_at=now() WHERE id=$1 AND state='running'")
                .bind(case_id).execute(&mut *tx).await.map_err(internal)?;
            append_case_event_tx(
                &mut tx,
                case_id,
                Some(work_item_id),
                Some(input.principal_id),
                "case.needs_attention",
                "supervisor review requested intervention or replanning",
                &input.output,
            )
            .await?;
        }
    }

    advance_case_tx(&mut tx, case_id).await?;
    let case_state = sqlx::query("SELECT state FROM company_cases WHERE id=$1")
        .bind(case_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?
        .get::<String, _>("state");
    tx.commit().await.map_err(internal)?;
    Ok(Json(
        serde_json::json!({"work_item_id":work_item_id,"state":"completed","case_state":case_state,"retry":false}),
    ))
}

async fn workplace_performance(State(state): State<AppState>) -> ApiResult {
    let rows = sqlx::query(
        r#"SELECT p.id,p.name,a.role,
                  COUNT(e.id)::bigint AS total,
                  COUNT(e.id) FILTER (WHERE e.outcome='succeeded')::bigint AS succeeded,
                  COUNT(e.id) FILTER (WHERE e.outcome='failed')::bigint AS failed,
                  COUNT(e.id) FILTER (WHERE e.outcome='retry')::bigint AS retries,
                  AVG(e.duration_ms)::float8 AS avg_duration_ms,
                  AVG(e.attempts)::float8 AS avg_attempts,
                  AVG(e.quality_score)::float8 AS avg_quality_score,
                  AVG(CASE WHEN e.sla_met IS TRUE THEN 1.0 WHEN e.sla_met IS FALSE THEN 0.0 END)::float8 AS sla_rate
           FROM principals p JOIN agents a ON a.principal_id=p.id
           LEFT JOIN agent_performance_events e ON e.principal_id=p.id
           WHERE p.status='active'
           GROUP BY p.id,p.name,a.role ORDER BY a.role,p.name"#,
    )
    .fetch_all(&state.db).await.map_err(internal)?;

    Ok(Json(Value::Array(rows.into_iter().map(|r| {
        let total = r.get::<i64,_>("total");
        let succeeded = r.get::<i64,_>("succeeded");
        serde_json::json!({
            "principal_id": r.get::<Uuid,_>("id"),
            "name": r.get::<String,_>("name"),
            "role": r.get::<String,_>("role"),
            "work_events": total,
            "succeeded": succeeded,
            "failed": r.get::<i64,_>("failed"),
            "retries": r.get::<i64,_>("retries"),
            "success_rate": if total > 0 { Some((succeeded as f64)/(total as f64)) } else { None::<f64> },
            "avg_duration_ms": r.get::<Option<f64>,_>("avg_duration_ms"),
            "avg_attempts": r.get::<Option<f64>,_>("avg_attempts"),
            "avg_quality_score": r.get::<Option<f64>,_>("avg_quality_score"),
            "sla_rate": r.get::<Option<f64>,_>("sla_rate")
        })
    }).collect())))
}

async fn reconcile_once(db: &PgPool) -> Result<(), (StatusCode, String)> {
    let expired = sqlx::query("SELECT id FROM case_work_items WHERE state='running' AND lease_expires_at <= now() ORDER BY lease_expires_at LIMIT 100")
        .fetch_all(db).await.map_err(internal)?;
    for r in expired {
        let id = r.get::<Uuid, _>("id");
        let mut tx = db.begin().await.map_err(internal)?;
        let row = sqlx::query("SELECT case_id,assigned_principal_id,attempt,max_attempts FROM case_work_items WHERE id=$1 AND state='running' AND lease_expires_at <= now() FOR UPDATE")
            .bind(id).fetch_optional(&mut *tx).await.map_err(internal)?;
        if let Some(row) = row {
            let case_id = row.get::<Uuid, _>("case_id");
            let principal = row.get::<Option<Uuid>, _>("assigned_principal_id");
            let attempt = row.get::<i32, _>("attempt");
            let max_attempts = row.get::<i32, _>("max_attempts");
            if attempt < max_attempts {
                sqlx::query("UPDATE case_work_items SET state='ready',assigned_principal_id=NULL,claimed_at=NULL,lease_expires_at=NULL WHERE id=$1")
                    .bind(id).execute(&mut *tx).await.map_err(internal)?;
                append_case_event_tx(
                    &mut tx,
                    case_id,
                    Some(id),
                    principal,
                    "work.lease_expired",
                    "work lease expired; bounded retry made item ready",
                    &serde_json::json!({"attempt":attempt}),
                )
                .await?;
            } else {
                sqlx::query("UPDATE case_work_items SET state='failed',completed_at=now(),lease_expires_at=NULL WHERE id=$1")
                    .bind(id).execute(&mut *tx).await.map_err(internal)?;
                sqlx::query("UPDATE company_cases SET state='needs_attention',updated_at=now() WHERE id=$1 AND state='running'")
                    .bind(case_id).execute(&mut *tx).await.map_err(internal)?;
                append_case_event_tx(
                    &mut tx,
                    case_id,
                    Some(id),
                    principal,
                    "case.needs_attention",
                    "work lease expired after maximum attempts",
                    &serde_json::json!({"attempt":attempt}),
                )
                .await?;
            }
        }
        tx.commit().await.map_err(internal)?;
    }

    let cases = sqlx::query(
        "SELECT id FROM company_cases WHERE state='running' ORDER BY updated_at LIMIT 200",
    )
    .fetch_all(db)
    .await
    .map_err(internal)?;
    for r in cases {
        let id = r.get::<Uuid, _>("id");
        let mut tx = db.begin().await.map_err(internal)?;
        advance_case_tx(&mut tx, id).await?;
        tx.commit().await.map_err(internal)?;
    }
    Ok(())
}

async fn advance_case_tx(
    tx: &mut Transaction<'_, Postgres>,
    case_id: Uuid,
) -> Result<(), (StatusCode, String)> {
    let case_state = sqlx::query("SELECT state FROM company_cases WHERE id=$1 FOR UPDATE")
        .bind(case_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "case not found".into()))?
        .get::<String, _>("state");
    if case_state != "running" {
        return Ok(());
    }

    for _ in 0..64 {
        let rows = sqlx::query(
            r#"SELECT id,node_key,kind,state,depends_on_json,condition_json,input_json,output_json,available_at
               FROM case_work_items WHERE case_id=$1 ORDER BY created_at,node_key"#,
        ).bind(case_id).fetch_all(&mut **tx).await.map_err(internal)?;

        let mut states = HashMap::<String, String>::new();
        let mut outputs = HashMap::<String, Value>::new();
        for r in &rows {
            states.insert(r.get::<String, _>("node_key"), r.get::<String, _>("state"));
            if let Some(text) = r.get::<Option<String>, _>("output_json") {
                outputs.insert(r.get::<String, _>("node_key"), parse_json(&text));
            }
        }

        let mut changed = false;
        for r in rows {
            let state = r.get::<String, _>("state");
            if !matches!(state.as_str(), "blocked" | "waiting") {
                continue;
            }
            let deps: Vec<String> =
                serde_json::from_str(&r.get::<String, _>("depends_on_json")).unwrap_or_default();
            if !deps.iter().all(|d| {
                matches!(
                    states.get(d).map(String::as_str),
                    Some("completed") | Some("skipped")
                )
            }) {
                continue;
            }

            let id = r.get::<Uuid, _>("id");
            let node_key = r.get::<String, _>("node_key");
            if let Some(condition_text) = r.get::<Option<String>, _>("condition_json") {
                let condition = parse_json(&condition_text);
                if !condition_matches(&condition, &outputs) {
                    sqlx::query("UPDATE case_work_items SET state='skipped',completed_at=now(),output_json='{}',lease_expires_at=NULL WHERE id=$1 AND state IN ('blocked','waiting')")
                        .bind(id).execute(&mut **tx).await.map_err(internal)?;
                    append_case_event_tx(
                        tx,
                        case_id,
                        Some(id),
                        None,
                        "work.skipped",
                        "workflow branch condition evaluated false",
                        &serde_json::json!({"node":node_key}),
                    )
                    .await?;
                    changed = true;
                    continue;
                }
            }

            let kind = r.get::<String, _>("kind");
            let input = parse_json(&r.get::<String, _>("input_json"));
            if kind == "timer" {
                let delay = input
                    .get("_delay_seconds")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .max(0);
                let available_at = r.get::<Option<DateTime<Utc>>, _>("available_at");
                match available_at {
                    None => {
                        sqlx::query("UPDATE case_work_items SET state='waiting',available_at=now()+($1::bigint * interval '1 second') WHERE id=$2")
                            .bind(delay).bind(id).execute(&mut **tx).await.map_err(internal)?;
                        changed = true;
                    }
                    Some(t) if t <= Utc::now() => {
                        sqlx::query("UPDATE case_work_items SET state='completed',completed_at=now(),output_json=$1 WHERE id=$2")
                            .bind(serde_json::json!({"timer_fired":true,"delay_seconds":delay}).to_string()).bind(id)
                            .execute(&mut **tx).await.map_err(internal)?;
                        append_case_event_tx(
                            tx,
                            case_id,
                            Some(id),
                            None,
                            "timer.fired",
                            "workflow timer completed",
                            &serde_json::json!({"node":node_key,"delay_seconds":delay}),
                        )
                        .await?;
                        changed = true;
                    }
                    _ => {}
                }
                continue;
            }

            if kind == "wait_event" {
                let wanted = input.get("_wait_for").and_then(Value::as_str).unwrap_or("");
                let seen = if wanted.is_empty() {
                    false
                } else {
                    sqlx::query("SELECT 1 FROM case_events WHERE case_id=$1 AND kind=$2 LIMIT 1")
                        .bind(case_id)
                        .bind(wanted)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_err(internal)?
                        .is_some()
                };
                if seen {
                    sqlx::query("UPDATE case_work_items SET state='completed',completed_at=now(),output_json=$1 WHERE id=$2")
                        .bind(serde_json::json!({"event_received":wanted}).to_string()).bind(id)
                        .execute(&mut **tx).await.map_err(internal)?;
                    append_case_event_tx(
                        tx,
                        case_id,
                        Some(id),
                        None,
                        "wait.satisfied",
                        "required business event was observed",
                        &serde_json::json!({"event":wanted}),
                    )
                    .await?;
                } else if state == "blocked" {
                    sqlx::query("UPDATE case_work_items SET state='waiting' WHERE id=$1")
                        .bind(id)
                        .execute(&mut **tx)
                        .await
                        .map_err(internal)?;
                    changed = true;
                }
                if seen {
                    changed = true;
                }
                continue;
            }

            let sla = input
                .get("_sla_seconds")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                .max(0);
            sqlx::query(
                "UPDATE case_work_items SET state='ready',available_at=COALESCE(available_at,now()),due_at=CASE WHEN $1::bigint > 0 THEN now()+($1::bigint * interval '1 second') ELSE due_at END WHERE id=$2 AND state IN ('blocked','waiting')",
            )
            .bind(sla).bind(id).execute(&mut **tx).await.map_err(internal)?;
            append_case_event_tx(
                tx,
                case_id,
                Some(id),
                None,
                "work.ready",
                "workflow dependencies satisfied; work item is ready",
                &serde_json::json!({"node":node_key}),
            )
            .await?;
            changed = true;
        }
        if !changed {
            break;
        }
    }

    let remaining = sqlx::query("SELECT COUNT(*)::bigint AS n FROM case_work_items WHERE case_id=$1 AND state NOT IN ('completed','skipped')")
        .bind(case_id).fetch_one(&mut **tx).await.map_err(internal)?.get::<i64,_>("n");
    if remaining == 0 {
        let updated = sqlx::query("UPDATE company_cases SET state='completed',actual_end_at=now(),updated_at=now() WHERE id=$1 AND state='running'")
            .bind(case_id).execute(&mut **tx).await.map_err(internal)?;
        if updated.rows_affected() == 1 {
            append_case_event_tx(
                tx,
                case_id,
                None,
                None,
                "case.completed",
                "definition of done satisfied; business case completed",
                &serde_json::json!({}),
            )
            .await?;
        }
    }
    Ok(())
}

fn condition_matches(condition: &Value, outputs: &HashMap<String, Value>) -> bool {
    let source = condition.get("source").and_then(Value::as_str);
    let path = condition.get("path").and_then(Value::as_str).unwrap_or("");
    let expected = condition.get("equals");
    let (Some(source), Some(expected)) = (source, expected) else {
        return false;
    };
    let Some(mut current) = outputs.get(source) else {
        return false;
    };
    for segment in path.split('.').filter(|s| !s.is_empty()) {
        let Some(next) = current.get(segment) else {
            return false;
        };
        current = next;
    }
    current == expected
}

fn validate_definition(def: &WorkflowDefinition) -> Result<(), (StatusCode, String)> {
    if def.nodes.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "workflow requires at least one node".into(),
        ));
    }
    let mut keys = HashSet::new();
    for node in &def.nodes {
        if node.key.trim().is_empty() || node.role.trim().is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "workflow node key and role cannot be empty".into(),
            ));
        }
        if !matches!(
            node.kind.as_str(),
            "task" | "supervisor" | "timer" | "wait_event"
        ) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("unsupported workflow node kind {}", node.kind),
            ));
        }
        if !keys.insert(node.key.clone()) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("duplicate workflow node key {}", node.key),
            ));
        }
        if node.kind == "wait_event" && node.wait_for.as_deref().unwrap_or("").is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("wait_event node {} requires wait_for", node.key),
            ));
        }
    }
    for node in &def.nodes {
        for dep in &node.depends_on {
            if dep == &node.key || !keys.contains(dep) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("node {} has invalid dependency {}", node.key, dep),
                ));
            }
        }
        if let Some(cond) = &node.condition {
            let source = cond.get("source").and_then(Value::as_str).unwrap_or("");
            if !keys.contains(source) || cond.get("equals").is_none() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("node {} has invalid condition", node.key),
                ));
            }
        }
    }

    let mut indegree: HashMap<String, usize> = def
        .nodes
        .iter()
        .map(|n| (n.key.clone(), n.depends_on.len()))
        .collect();
    let mut queue: Vec<String> = indegree
        .iter()
        .filter(|(_, v)| **v == 0)
        .map(|(k, _)| k.clone())
        .collect();
    let mut visited = 0usize;
    while let Some(k) = queue.pop() {
        visited += 1;
        for n in &def.nodes {
            if n.depends_on.contains(&k) {
                if let Some(v) = indegree.get_mut(&n.key) {
                    *v -= 1;
                    if *v == 0 {
                        queue.push(n.key.clone());
                    }
                }
            }
        }
    }
    if visited != def.nodes.len() {
        return Err((
            StatusCode::BAD_REQUEST,
            "workflow dependency graph contains a cycle".into(),
        ));
    }
    Ok(())
}

async fn active_agent_role(
    db: &PgPool,
    principal_id: Uuid,
) -> Result<String, (StatusCode, String)> {
    let row = sqlx::query(
        r#"SELECT a.role FROM agents a JOIN principals p ON p.id=a.principal_id
           WHERE a.principal_id=$1 AND p.kind='agent' AND p.status='active'"#,
    )
    .bind(principal_id)
    .fetch_optional(db)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::FORBIDDEN, "active agent required".into()))?;
    Ok(row.get::<String, _>("role"))
}

async fn ensure_active_principal(
    db: &PgPool,
    principal_id: Uuid,
) -> Result<(), (StatusCode, String)> {
    let ok = sqlx::query("SELECT 1 FROM principals WHERE id=$1 AND status='active'")
        .bind(principal_id)
        .fetch_optional(db)
        .await
        .map_err(internal)?
        .is_some();
    if ok {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "active principal required".into()))
    }
}

async fn ensure_role_tx(
    tx: &mut Transaction<'_, Postgres>,
    principal_id: Uuid,
    role: &str,
) -> Result<(), (StatusCode, String)> {
    let ok = sqlx::query(
        r#"SELECT 1 FROM agents a JOIN principals p ON p.id=a.principal_id
           WHERE a.principal_id=$1 AND a.role=$2 AND p.status='active'"#,
    )
    .bind(principal_id)
    .bind(role)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?
    .is_some();
    if ok {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            format!("active {role} agent required"),
        ))
    }
}

async fn ensure_capability_tx(
    tx: &mut Transaction<'_, Postgres>,
    principal_id: Uuid,
    capability: &str,
) -> Result<(), (StatusCode, String)> {
    let ok = sqlx::query(
        r#"SELECT 1 FROM principal_capabilities pc JOIN capabilities c ON c.id=pc.capability_id
           JOIN principals p ON p.id=pc.principal_id
           WHERE pc.principal_id=$1 AND c.name=$2 AND p.status='active'"#,
    )
    .bind(principal_id)
    .bind(capability)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?
    .is_some();
    if ok {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            format!("missing capability {capability}"),
        ))
    }
}

async fn ensure_case_exists_tx(
    tx: &mut Transaction<'_, Postgres>,
    case_id: Uuid,
) -> Result<(), (StatusCode, String)> {
    if sqlx::query("SELECT 1 FROM company_cases WHERE id=$1")
        .bind(case_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?
        .is_some()
    {
        Ok(())
    } else {
        Err((StatusCode::NOT_FOUND, "case not found".into()))
    }
}

async fn append_case_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    case_id: Uuid,
    work_item_id: Option<Uuid>,
    principal_id: Option<Uuid>,
    kind: &str,
    message: &str,
    payload: &Value,
) -> Result<(), (StatusCode, String)> {
    sqlx::query(
        "INSERT INTO case_events(id,case_id,work_item_id,principal_id,kind,message,payload_json,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,now())",
    ).bind(Uuid::new_v4()).bind(case_id).bind(work_item_id).bind(principal_id).bind(kind).bind(message).bind(payload.to_string())
      .execute(&mut **tx).await.map_err(internal)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn record_performance_tx(
    tx: &mut Transaction<'_, Postgres>,
    principal_id: Uuid,
    case_id: Uuid,
    work_item_id: Uuid,
    role: &str,
    outcome: &str,
    duration_ms: Option<i64>,
    attempts: i32,
    quality_score: Option<f64>,
    sla_met: Option<bool>,
) -> Result<(), (StatusCode, String)> {
    sqlx::query(
        r#"INSERT INTO agent_performance_events
           (id,principal_id,case_id,work_item_id,role,outcome,duration_ms,attempts,quality_score,sla_met,created_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,now())"#,
    ).bind(Uuid::new_v4()).bind(principal_id).bind(case_id).bind(work_item_id).bind(role).bind(outcome)
      .bind(duration_ms).bind(attempts).bind(quality_score).bind(sla_met)
      .execute(&mut **tx).await.map_err(internal)?;
    Ok(())
}

fn case_summary_json(r: sqlx::postgres::PgRow) -> Value {
    serde_json::json!({
        "id": r.get::<Uuid,_>("id"), "case_type": r.get::<String,_>("case_type"), "title": r.get::<String,_>("title"),
        "state": r.get::<String,_>("state"), "workflow_definition_id": r.get::<Uuid,_>("workflow_definition_id"),
        "workflow_version": r.get::<i32,_>("workflow_version"), "environment": r.get::<String,_>("environment"),
        "priority": r.get::<i32,_>("priority"), "budget_amount": r.get::<Option<f64>,_>("budget_amount"),
        "budget_currency": r.get::<Option<String>,_>("budget_currency"),
        "planned_start_at": r.get::<Option<DateTime<Utc>>,_>("planned_start_at"), "planned_end_at": r.get::<Option<DateTime<Utc>>,_>("planned_end_at"),
        "actual_start_at": r.get::<DateTime<Utc>,_>("actual_start_at"), "actual_end_at": r.get::<Option<DateTime<Utc>>,_>("actual_end_at"),
        "created_by_principal_id": r.get::<Uuid,_>("created_by_principal_id"), "created_at": r.get::<DateTime<Utc>,_>("created_at"),
        "updated_at": r.get::<DateTime<Utc>,_>("updated_at")
    })
}

fn work_item_json(r: sqlx::postgres::PgRow) -> Value {
    serde_json::json!({
        "id": r.get::<Uuid,_>("id"), "case_id": r.get::<Uuid,_>("case_id"), "node_key": r.get::<String,_>("node_key"),
        "kind": r.get::<String,_>("kind"), "role": r.get::<String,_>("role"), "state": r.get::<String,_>("state"),
        "required": r.get::<bool,_>("required"), "attempt": r.get::<i32,_>("attempt"), "max_attempts": r.get::<i32,_>("max_attempts"),
        "depends_on": parse_json(&r.get::<String,_>("depends_on_json")),
        "condition": r.get::<Option<String>,_>("condition_json").map(|s| parse_json(&s)),
        "input": parse_json(&r.get::<String,_>("input_json")),
        "output": r.get::<Option<String>,_>("output_json").map(|s| parse_json(&s)),
        "checklist": parse_json(&r.get::<String,_>("checklist_json")),
        "assigned_principal_id": r.get::<Option<Uuid>,_>("assigned_principal_id"),
        "claimed_at": r.get::<Option<DateTime<Utc>>,_>("claimed_at"), "lease_expires_at": r.get::<Option<DateTime<Utc>>,_>("lease_expires_at"),
        "available_at": r.get::<Option<DateTime<Utc>>,_>("available_at"), "due_at": r.get::<Option<DateTime<Utc>>,_>("due_at"),
        "completed_at": r.get::<Option<DateTime<Utc>>,_>("completed_at"), "created_at": r.get::<DateTime<Utc>,_>("created_at")
    })
}

fn parse_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| serde_json::json!({"raw": text}))
}

fn internal<E: std::fmt::Display>(error: E) -> (StatusCode, String) {
    crate::security::internal_error(error)
}

type ApiResult = Result<Json<Value>, (StatusCode, String)>;
