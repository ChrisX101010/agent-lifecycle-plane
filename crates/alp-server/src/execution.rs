use crate::AppState;
use alp_core::{LocalRuntime, RiskLevel, RuntimeAdapter, RuntimeKind, Task, TaskState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct LeaseTask {
    worker_principal_id: Uuid,
    authorized_by_principal_id: Uuid,
    ttl_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ExecuteTask {
    worker_principal_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct CompleteTask {
    worker_principal_id: Uuid,
    receipt_id: Uuid,
    outcome: String,
    output_summary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VerifyTask {
    verifier_principal_id: Uuid,
    receipt_id: Uuid,
    decision: String,
    reason: Option<String>,
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/tasks/{id}/lease", post(lease_task))
        .route("/v1/tasks/{id}/execute", post(execute_task))
        .route("/v1/tasks/{id}/complete", post(complete_task))
        .route("/v1/tasks/{id}/verify", post(verify_task))
        .route(
            "/v1/runtime/agents/{id}/recovery",
            get(agent_recovery_state),
        )
}

async fn lease_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<LeaseTask>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    ensure_principal_kind(&state.db, input.authorized_by_principal_id, "user").await?;
    ensure_agent_role(&state.db, input.worker_principal_id, "worker").await?;
    ensure_capability(&state.db, input.worker_principal_id, "task.execute").await?;

    let ttl_seconds = input.ttl_seconds.unwrap_or(300).clamp(30, 3600);
    let mut tx = state.db.begin().await.map_err(internal)?;

    let row = sqlx::query("SELECT state FROM tasks WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "task not found".into()))?;

    let task_state = row.get::<String, _>("state");
    if task_state != "queued" && task_state != "approved" {
        return Err((
            StatusCode::CONFLICT,
            format!("task cannot be leased from state {task_state}"),
        ));
    }

    sqlx::query(
        r#"
        UPDATE task_leases
        SET state='expired', released_at=now()
        WHERE task_id=$1 AND state='active' AND expires_at <= now()
        "#,
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    let active = sqlx::query(
        "SELECT id FROM task_leases WHERE task_id=$1 AND state='active' AND expires_at > now()",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal)?;

    if active.is_some() {
        return Err((
            StatusCode::CONFLICT,
            "task already has an active lease".into(),
        ));
    }

    let lease_id = Uuid::new_v4();
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(ttl_seconds);

    sqlx::query(
        r#"
        INSERT INTO task_leases
          (id,task_id,worker_principal_id,authorized_by_principal_id,state,acquired_at,expires_at)
        VALUES ($1,$2,$3,$4,'active',now(),$5)
        "#,
    )
    .bind(lease_id)
    .bind(id)
    .bind(input.worker_principal_id)
    .bind(input.authorized_by_principal_id)
    .bind(expires_at)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    tx.commit().await.map_err(internal)?;

    append_event_as(
        &state.db,
        Some(id),
        Some(input.authorized_by_principal_id),
        "task.leased",
        "human owner authorized execution lease",
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "lease_id": lease_id,
            "task_id": id,
            "worker_principal_id": input.worker_principal_id,
            "state": "active",
            "expires_at": expires_at
        })),
    ))
}

async fn execute_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<ExecuteTask>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    ensure_agent_role(&state.db, input.worker_principal_id, "worker").await?;
    ensure_capability(&state.db, input.worker_principal_id, "task.execute").await?;
    ensure_active_lease(&state.db, id, input.worker_principal_id).await?;

    let row = sqlx::query(
        r#"
        SELECT id,name,description,risk,runtime,state,created_at,updated_at
        FROM tasks WHERE id=$1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "task not found".into()))?;

    let task = task_from_row(row)?;

    if task.state != TaskState::Queued && task.state != TaskState::Approved {
        return Err((
            StatusCode::CONFLICT,
            format!("task cannot execute from state {}", state_name(task.state)),
        ));
    }

    let receipt = match task.runtime {
        RuntimeKind::Local => LocalRuntime
            .submit(&task)
            .await
            .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?,
        _ => {
            return Err((
                StatusCode::NOT_IMPLEMENTED,
                format!(
                    "runtime {} adapter is not wired yet",
                    runtime_name(task.runtime)
                ),
            ))
        }
    };

    let receipt_id = Uuid::new_v4();
    let mut tx = state.db.begin().await.map_err(internal)?;

    let updated = sqlx::query(
        r#"
        UPDATE tasks SET state='running', updated_at=now()
        WHERE id=$1 AND state IN ('queued','approved')
        "#,
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    if updated.rows_affected() != 1 {
        return Err((
            StatusCode::CONFLICT,
            "task state changed before execution".into(),
        ));
    }

    sqlx::query(
        r#"
        INSERT INTO execution_receipts
          (id,task_id,worker_principal_id,runtime,external_id,status,accepted_at,created_at)
        VALUES ($1,$2,$3,$4,$5,'accepted',$6,now())
        "#,
    )
    .bind(receipt_id)
    .bind(id)
    .bind(input.worker_principal_id)
    .bind(runtime_name(receipt.runtime))
    .bind(&receipt.external_id)
    .bind(receipt.accepted_at)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    tx.commit().await.map_err(internal)?;

    append_event_as(
        &state.db,
        Some(id),
        Some(input.worker_principal_id),
        "task.execution.accepted",
        "runtime accepted task execution",
    )
    .await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "receipt_id": receipt_id,
            "task_id": id,
            "worker_principal_id": input.worker_principal_id,
            "runtime": runtime_name(receipt.runtime),
            "external_id": receipt.external_id,
            "state": "running",
            "accepted_at": receipt.accepted_at
        })),
    ))
}

async fn complete_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<CompleteTask>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    ensure_agent_role(&state.db, input.worker_principal_id, "worker").await?;
    ensure_capability(&state.db, input.worker_principal_id, "task.execute").await?;
    ensure_active_lease(&state.db, id, input.worker_principal_id).await?;

    let (receipt_status, task_state) = match input.outcome.as_str() {
        "succeeded" => ("completed", "submitted_unverified"),
        "failed" => ("failed", "failed"),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "outcome must be succeeded or failed".into(),
            ))
        }
    };

    let mut tx = state.db.begin().await.map_err(internal)?;

    let receipt = sqlx::query(
        r#"
        SELECT worker_principal_id,status
        FROM execution_receipts
        WHERE id=$1 AND task_id=$2
        FOR UPDATE
        "#,
    )
    .bind(input.receipt_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "execution receipt not found".into()))?;

    if receipt.get::<Uuid, _>("worker_principal_id") != input.worker_principal_id {
        return Err((
            StatusCode::FORBIDDEN,
            "receipt belongs to another worker".into(),
        ));
    }

    if receipt.get::<String, _>("status") != "accepted" {
        return Err((
            StatusCode::CONFLICT,
            "receipt is not awaiting completion".into(),
        ));
    }

    sqlx::query(
        r#"
        UPDATE execution_receipts
        SET status=$1,output_summary=$2,completed_at=now()
        WHERE id=$3
        "#,
    )
    .bind(receipt_status)
    .bind(&input.output_summary)
    .bind(input.receipt_id)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    let updated =
        sqlx::query("UPDATE tasks SET state=$1,updated_at=now() WHERE id=$2 AND state='running'")
            .bind(task_state)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;

    if updated.rows_affected() != 1 {
        return Err((StatusCode::CONFLICT, "task is not running".into()));
    }

    sqlx::query(
        r#"
        UPDATE task_leases
        SET state='released',released_at=now()
        WHERE task_id=$1 AND worker_principal_id=$2 AND state='active'
        "#,
    )
    .bind(id)
    .bind(input.worker_principal_id)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    tx.commit().await.map_err(internal)?;

    append_event_as(
        &state.db,
        Some(id),
        Some(input.worker_principal_id),
        "task.execution.completed",
        if input.outcome == "succeeded" {
            "worker submitted outcome for independent verification"
        } else {
            "worker reported task failure"
        },
    )
    .await?;

    Ok(Json(serde_json::json!({
        "task_id": id,
        "receipt_id": input.receipt_id,
        "state": task_state,
        "outcome": input.outcome
    })))
}

async fn verify_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<VerifyTask>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    ensure_agent_role(&state.db, input.verifier_principal_id, "verifier").await?;
    ensure_capability(&state.db, input.verifier_principal_id, "outcome.verify").await?;

    let final_state = match input.decision.as_str() {
        "verified" => "verified",
        "rejected" => "verification_rejected",
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "decision must be verified or rejected".into(),
            ))
        }
    };

    let mut tx = state.db.begin().await.map_err(internal)?;

    let receipt = sqlx::query(
        r#"
        SELECT worker_principal_id,status
        FROM execution_receipts
        WHERE id=$1 AND task_id=$2
        "#,
    )
    .bind(input.receipt_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "execution receipt not found".into()))?;

    let worker_principal_id = receipt.get::<Uuid, _>("worker_principal_id");
    if worker_principal_id == input.verifier_principal_id {
        return Err((
            StatusCode::FORBIDDEN,
            "worker cannot verify its own outcome".into(),
        ));
    }

    if receipt.get::<String, _>("status") != "completed" {
        return Err((StatusCode::CONFLICT, "receipt is not completed".into()));
    }

    let task_row = sqlx::query("SELECT state FROM tasks WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;

    if task_row.get::<String, _>("state") != "submitted_unverified" {
        return Err((
            StatusCode::CONFLICT,
            "task is not awaiting independent verification".into(),
        ));
    }

    sqlx::query(
        r#"
        INSERT INTO verifications
          (id,task_id,receipt_id,verifier_principal_id,decision,reason,created_at)
        VALUES ($1,$2,$3,$4,$5,$6,now())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .bind(input.receipt_id)
    .bind(input.verifier_principal_id)
    .bind(&input.decision)
    .bind(&input.reason)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    sqlx::query("UPDATE tasks SET state=$1,updated_at=now() WHERE id=$2")
        .bind(final_state)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;

    tx.commit().await.map_err(internal)?;

    append_event_as(
        &state.db,
        Some(id),
        Some(input.verifier_principal_id),
        "task.verified",
        if input.decision == "verified" {
            "independent verifier accepted outcome"
        } else {
            "independent verifier rejected outcome"
        },
    )
    .await?;

    Ok(Json(serde_json::json!({
        "task_id": id,
        "receipt_id": input.receipt_id,
        "verifier_principal_id": input.verifier_principal_id,
        "decision": input.decision,
        "state": final_state
    })))
}

async fn agent_recovery_state(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let agent = sqlx::query(
        r#"
        SELECT p.name,a.role,a.runtime
        FROM principals p
        JOIN agents a ON a.principal_id=p.id
        WHERE p.id=$1 AND p.kind='agent' AND p.status='active'
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "active agent not found".into()))?;

    let lease = sqlx::query(
        r#"
        SELECT tl.id AS lease_id,
               tl.task_id,
               tl.state AS lease_state,
               tl.expires_at,
               t.state AS task_state
        FROM task_leases tl
        JOIN tasks t ON t.id=tl.task_id
        WHERE tl.worker_principal_id=$1
          AND tl.state='active'
          AND tl.expires_at > now()
        ORDER BY tl.acquired_at DESC
        LIMIT 1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?;

    let active_lease = lease.map(|row| {
        serde_json::json!({
            "lease_id": row.get::<Uuid,_>("lease_id"),
            "task_id": row.get::<Uuid,_>("task_id"),
            "lease_state": row.get::<String,_>("lease_state"),
            "task_state": row.get::<String,_>("task_state"),
            "expires_at": row.get::<chrono::DateTime<chrono::Utc>,_>("expires_at")
        })
    });

    Ok(Json(serde_json::json!({
        "principal_id": id,
        "name": agent.get::<String,_>("name"),
        "role": agent.get::<String,_>("role"),
        "agent_runtime": agent.get::<String,_>("runtime"),
        "active_lease": active_lease
    })))
}

pub(crate) async fn ensure_principal_kind(
    db: &PgPool,
    principal_id: Uuid,
    kind: &str,
) -> Result<(), (StatusCode, String)> {
    let ok = sqlx::query("SELECT 1 FROM principals WHERE id=$1 AND kind=$2 AND status='active'")
        .bind(principal_id)
        .bind(kind)
        .fetch_optional(db)
        .await
        .map_err(internal)?
        .is_some();

    if !ok {
        return Err((
            StatusCode::FORBIDDEN,
            format!("active {kind} principal required"),
        ));
    }

    Ok(())
}

pub(crate) async fn ensure_agent_role(
    db: &PgPool,
    principal_id: Uuid,
    role: &str,
) -> Result<(), (StatusCode, String)> {
    let ok = sqlx::query(
        r#"
        SELECT 1
        FROM principals p
        JOIN agents a ON a.principal_id=p.id
        WHERE p.id=$1 AND p.kind='agent' AND p.status='active' AND a.role=$2
        "#,
    )
    .bind(principal_id)
    .bind(role)
    .fetch_optional(db)
    .await
    .map_err(internal)?
    .is_some();

    if !ok {
        return Err((
            StatusCode::FORBIDDEN,
            format!("active {role} agent required"),
        ));
    }

    Ok(())
}

pub(crate) async fn ensure_capability(
    db: &PgPool,
    principal_id: Uuid,
    capability: &str,
) -> Result<(), (StatusCode, String)> {
    let ok = sqlx::query(
        r#"
        SELECT 1
        FROM principal_capabilities pc
        JOIN capabilities c ON c.id=pc.capability_id
        WHERE pc.principal_id=$1 AND c.name=$2
        "#,
    )
    .bind(principal_id)
    .bind(capability)
    .fetch_optional(db)
    .await
    .map_err(internal)?
    .is_some();

    if !ok {
        return Err((
            StatusCode::FORBIDDEN,
            format!("principal lacks capability {capability}"),
        ));
    }

    Ok(())
}

async fn ensure_active_lease(
    db: &PgPool,
    task_id: Uuid,
    worker_principal_id: Uuid,
) -> Result<(), (StatusCode, String)> {
    let ok = sqlx::query(
        r#"
        SELECT 1 FROM task_leases
        WHERE task_id=$1
          AND worker_principal_id=$2
          AND state='active'
          AND expires_at > now()
        "#,
    )
    .bind(task_id)
    .bind(worker_principal_id)
    .fetch_optional(db)
    .await
    .map_err(internal)?
    .is_some();

    if !ok {
        return Err((
            StatusCode::FORBIDDEN,
            "active execution lease required".into(),
        ));
    }

    Ok(())
}

pub(crate) async fn append_event_as(
    db: &PgPool,
    task_id: Option<Uuid>,
    principal_id: Option<Uuid>,
    kind: &str,
    message: &str,
) -> Result<(), (StatusCode, String)> {
    sqlx::query(
        r#"
        INSERT INTO audit_events
          (id,task_id,principal_id,kind,message,created_at)
        VALUES ($1,$2,$3,$4,$5,now())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(task_id)
    .bind(principal_id)
    .bind(kind)
    .bind(message)
    .execute(db)
    .await
    .map_err(internal)?;

    Ok(())
}

fn task_from_row(row: sqlx::postgres::PgRow) -> Result<Task, (StatusCode, String)> {
    Ok(Task {
        id: row.get::<Uuid, _>("id"),
        name: row.get::<String, _>("name"),
        description: row.get::<String, _>("description"),
        risk: parse_risk(&row.get::<String, _>("risk"))?,
        runtime: parse_runtime(&row.get::<String, _>("runtime"))?,
        state: parse_state(&row.get::<String, _>("state"))?,
        created_at: row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
        updated_at: row.get::<chrono::DateTime<chrono::Utc>, _>("updated_at"),
    })
}

fn parse_risk(value: &str) -> Result<RiskLevel, (StatusCode, String)> {
    match value {
        "low" => Ok(RiskLevel::Low),
        "medium" => Ok(RiskLevel::Medium),
        "high" => Ok(RiskLevel::High),
        "critical" => Ok(RiskLevel::Critical),
        _ => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unknown risk {value}"),
        )),
    }
}

fn parse_runtime(value: &str) -> Result<RuntimeKind, (StatusCode, String)> {
    match value {
        "local" => Ok(RuntimeKind::Local),
        "docker" => Ok(RuntimeKind::Docker),
        "kubernetes" => Ok(RuntimeKind::Kubernetes),
        "serverless" => Ok(RuntimeKind::Serverless),
        "edge" => Ok(RuntimeKind::Edge),
        "floci_aws" => Ok(RuntimeKind::FlociAws),
        "floci_azure" => Ok(RuntimeKind::FlociAzure),
        _ => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unknown runtime {value}"),
        )),
    }
}

fn parse_state(value: &str) -> Result<TaskState, (StatusCode, String)> {
    match value {
        "queued" => Ok(TaskState::Queued),
        "awaiting_approval" => Ok(TaskState::AwaitingApproval),
        "approved" => Ok(TaskState::Approved),
        "running" => Ok(TaskState::Running),
        "submitted_unverified" => Ok(TaskState::SubmittedUnverified),
        "verified" => Ok(TaskState::Verified),
        "verification_rejected" => Ok(TaskState::VerificationRejected),
        "succeeded" => Ok(TaskState::Succeeded),
        "failed" => Ok(TaskState::Failed),
        "cancelled" => Ok(TaskState::Cancelled),
        _ => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unknown task state {value}"),
        )),
    }
}

fn runtime_name(runtime: RuntimeKind) -> &'static str {
    match runtime {
        RuntimeKind::Local => "local",
        RuntimeKind::Docker => "docker",
        RuntimeKind::Kubernetes => "kubernetes",
        RuntimeKind::Serverless => "serverless",
        RuntimeKind::Edge => "edge",
        RuntimeKind::FlociAws => "floci_aws",
        RuntimeKind::FlociAzure => "floci_azure",
    }
}

fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Queued => "queued",
        TaskState::AwaitingApproval => "awaiting_approval",
        TaskState::Approved => "approved",
        TaskState::Running => "running",
        TaskState::SubmittedUnverified => "submitted_unverified",
        TaskState::Verified => "verified",
        TaskState::VerificationRejected => "verification_rejected",
        TaskState::Succeeded => "succeeded",
        TaskState::Failed => "failed",
        TaskState::Cancelled => "cancelled",
    }
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
