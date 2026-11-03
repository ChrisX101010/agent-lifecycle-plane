use crate::{execution, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use sqlx::{PgPool, Row};
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct CreateOrchestration {
    goal: String,
    created_by_principal_id: Uuid,
    max_attempts: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct CompleteStep {
    principal_id: Uuid,
    status: String,
    output: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct HeartbeatStep {
    principal_id: Uuid,
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/orchestrations",
            post(create_orchestration).get(list_orchestrations),
        )
        .route("/v1/orchestrations/{id}", get(get_orchestration))
        .route("/v1/orchestration/agents/{id}/claim", post(claim_next_step))
        .route(
            "/v1/orchestration/steps/{id}/heartbeat",
            post(heartbeat_step),
        )
        .route("/v1/orchestration/steps/{id}/complete", post(complete_step))
}

pub(crate) fn spawn_reconciler(db: PgPool) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = reconcile_once(&db).await {
                tracing::error!(%error, "ALP reconciliation pass failed");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

async fn create_orchestration(
    State(state): State<AppState>,
    Json(input): Json<CreateOrchestration>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    execution::ensure_agent_role(&state.db, input.created_by_principal_id, "planner").await?;
    execution::ensure_capability(&state.db, input.created_by_principal_id, "plan.create").await?;

    let goal = input.goal.trim();
    if goal.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "goal cannot be empty".into()));
    }

    let max_attempts = input.max_attempts.unwrap_or(3).clamp(1, 5);
    let id = Uuid::new_v4();
    let mut tx = state.db.begin().await.map_err(internal)?;

    sqlx::query(
        r#"
        INSERT INTO orchestrations
          (id,goal,state,current_seq,created_by_principal_id,created_at,updated_at)
        VALUES ($1,$2,'running',1,$3,now(),now())
        "#,
    )
    .bind(id)
    .bind(goal)
    .bind(input.created_by_principal_id)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    for (index, role) in ["planner", "researcher", "worker", "verifier"]
        .iter()
        .enumerate()
    {
        let seq = index as i32 + 1;
        let step_id = Uuid::new_v4();
        let initial_input = if seq == 1 {
            serde_json::json!({"goal": goal, "mode": "governed_orchestration"}).to_string()
        } else {
            "{}".into()
        };

        sqlx::query(
            r#"
            INSERT INTO orchestration_steps
              (id,orchestration_id,seq,role,state,attempt,max_attempts,input_json,created_at)
            VALUES ($1,$2,$3,$4,'pending',0,$5,$6,now())
            "#,
        )
        .bind(step_id)
        .bind(id)
        .bind(seq)
        .bind(*role)
        .bind(max_attempts)
        .bind(initial_input)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    }

    append_event_tx(
        &mut tx,
        id,
        None,
        Some(input.created_by_principal_id),
        "orchestration.created",
        "planner created governed orchestration",
    )
    .await?;

    tx.commit().await.map_err(internal)?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": id,
            "goal": goal,
            "state": "running",
            "current_seq": 1,
            "max_attempts": max_attempts
        })),
    ))
}

async fn list_orchestrations(
    State(state): State<AppState>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let rows = sqlx::query(
        r#"
        SELECT id,goal,state,current_seq,created_by_principal_id,created_at,updated_at
        FROM orchestrations
        ORDER BY created_at DESC
        LIMIT 100
        "#,
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(
        rows.into_iter()
            .map(|row| {
                serde_json::json!({
                    "id": row.get::<Uuid,_>("id"),
                    "goal": row.get::<String,_>("goal"),
                    "state": row.get::<String,_>("state"),
                    "current_seq": row.get::<i32,_>("current_seq"),
                    "created_by_principal_id": row.get::<Uuid,_>("created_by_principal_id"),
                    "created_at": row.get::<chrono::DateTime<chrono::Utc>,_>("created_at"),
                    "updated_at": row.get::<chrono::DateTime<chrono::Utc>,_>("updated_at")
                })
            })
            .collect(),
    ))
}

async fn get_orchestration(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let o = sqlx::query(
        r#"
        SELECT id,goal,state,current_seq,created_by_principal_id,created_at,updated_at
        FROM orchestrations WHERE id=$1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "orchestration not found".into()))?;

    let steps = sqlx::query(
        r#"
        SELECT id,orchestration_id,seq,role,state,assigned_principal_id,attempt,max_attempts,
               input_json,output_json,claimed_at,lease_expires_at,completed_at,created_at
        FROM orchestration_steps WHERE orchestration_id=$1 ORDER BY seq
        "#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    let handoffs = sqlx::query(
        r#"
        SELECT id,from_step_id,to_step_id,from_principal_id,to_principal_id,payload_json,created_at
        FROM orchestration_handoffs WHERE orchestration_id=$1 ORDER BY created_at
        "#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    let events = sqlx::query(
        r#"
        SELECT id,step_id,principal_id,kind,message,created_at
        FROM orchestration_events WHERE orchestration_id=$1 ORDER BY created_at
        "#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    let step_json: Vec<_> = steps.into_iter().map(step_json).collect();
    let handoff_json: Vec<_> = handoffs
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "id": row.get::<Uuid,_>("id"),
                "from_step_id": row.get::<Uuid,_>("from_step_id"),
                "to_step_id": row.get::<Uuid,_>("to_step_id"),
                "from_principal_id": row.get::<Uuid,_>("from_principal_id"),
                "to_principal_id": row.get::<Option<Uuid>,_>("to_principal_id"),
                "payload": parse_json(&row.get::<String,_>("payload_json")),
                "created_at": row.get::<chrono::DateTime<chrono::Utc>,_>("created_at")
            })
        })
        .collect();

    let event_json: Vec<_> = events
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "id": row.get::<Uuid,_>("id"),
                "step_id": row.get::<Option<Uuid>,_>("step_id"),
                "principal_id": row.get::<Option<Uuid>,_>("principal_id"),
                "kind": row.get::<String,_>("kind"),
                "message": row.get::<String,_>("message"),
                "created_at": row.get::<chrono::DateTime<chrono::Utc>,_>("created_at")
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "orchestration": {
            "id": o.get::<Uuid,_>("id"),
            "goal": o.get::<String,_>("goal"),
            "state": o.get::<String,_>("state"),
            "current_seq": o.get::<i32,_>("current_seq"),
            "created_by_principal_id": o.get::<Uuid,_>("created_by_principal_id"),
            "created_at": o.get::<chrono::DateTime<chrono::Utc>,_>("created_at"),
            "updated_at": o.get::<chrono::DateTime<chrono::Utc>,_>("updated_at")
        },
        "steps": step_json,
        "handoffs": handoff_json,
        "events": event_json
    })))
}

async fn claim_next_step(
    State(state): State<AppState>,
    Path(principal_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let role = active_agent_role(&state.db, principal_id).await?;
    execution::ensure_capability(&state.db, principal_id, capability_for_role(&role)?).await?;

    let mut tx = state.db.begin().await.map_err(internal)?;

    let existing = sqlx::query(
        r#"
        SELECT s.id,s.orchestration_id,s.seq,s.role,s.state,s.assigned_principal_id,
               s.attempt,s.max_attempts,s.input_json,s.output_json,s.claimed_at,
               s.lease_expires_at,s.completed_at,s.created_at
        FROM orchestration_steps s
        JOIN orchestrations o ON o.id=s.orchestration_id
        WHERE o.state='running' AND s.state='running'
          AND s.assigned_principal_id=$1
          AND s.lease_expires_at > now()
          AND s.seq=o.current_seq
        ORDER BY s.claimed_at
        LIMIT 1
        FOR UPDATE OF s SKIP LOCKED
        "#,
    )
    .bind(principal_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal)?;

    if let Some(row) = existing {
        tx.commit().await.map_err(internal)?;
        return Ok(Json(serde_json::json!({
            "state": "claimed",
            "resumed": true,
            "step": step_json(row)
        })));
    }

    let candidate = sqlx::query(
        r#"
        SELECT s.id
        FROM orchestration_steps s
        JOIN orchestrations o ON o.id=s.orchestration_id
        WHERE o.state='running' AND s.state='pending'
          AND s.role=$1 AND s.seq=o.current_seq
        ORDER BY o.created_at,s.seq
        LIMIT 1
        FOR UPDATE OF s SKIP LOCKED
        "#,
    )
    .bind(&role)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal)?;

    let Some(candidate) = candidate else {
        tx.commit().await.map_err(internal)?;
        return Ok(Json(serde_json::json!({"state": "no_work"})));
    };

    let step_id = candidate.get::<Uuid, _>("id");
    let row = sqlx::query(
        r#"
        UPDATE orchestration_steps
        SET state='running',assigned_principal_id=$1,attempt=attempt+1,
            claimed_at=now(),lease_expires_at=now()+interval '180 seconds'
        WHERE id=$2 AND state='pending'
        RETURNING id,orchestration_id,seq,role,state,assigned_principal_id,
                  attempt,max_attempts,input_json,output_json,claimed_at,
                  lease_expires_at,completed_at,created_at
        "#,
    )
    .bind(principal_id)
    .bind(step_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(internal)?;

    let orchestration_id = row.get::<Uuid, _>("orchestration_id");

    sqlx::query(
        "UPDATE orchestration_handoffs SET to_principal_id=$1 WHERE to_step_id=$2 AND to_principal_id IS NULL",
    )
    .bind(principal_id)
    .bind(step_id)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    append_event_tx(
        &mut tx,
        orchestration_id,
        Some(step_id),
        Some(principal_id),
        "orchestration.step.claimed",
        "agent claimed durable orchestration step",
    )
    .await?;

    tx.commit().await.map_err(internal)?;

    Ok(Json(serde_json::json!({
        "state": "claimed",
        "resumed": false,
        "step": step_json(row)
    })))
}

async fn heartbeat_step(
    State(state): State<AppState>,
    Path(step_id): Path<Uuid>,
    Json(input): Json<HeartbeatStep>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let role = active_agent_role(&state.db, input.principal_id).await?;
    execution::ensure_capability(&state.db, input.principal_id, capability_for_role(&role)?)
        .await?;

    let row = sqlx::query(
        r#"
        UPDATE orchestration_steps
        SET lease_expires_at=now()+interval '180 seconds'
        WHERE id=$1 AND state='running' AND assigned_principal_id=$2
          AND lease_expires_at > now()
        RETURNING orchestration_id,lease_expires_at
        "#,
    )
    .bind(step_id)
    .bind(input.principal_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or((
        StatusCode::CONFLICT,
        "step has no renewable active lease for this principal".into(),
    ))?;

    Ok(Json(serde_json::json!({
        "step_id": step_id,
        "orchestration_id": row.get::<Uuid,_>("orchestration_id"),
        "state": "renewed",
        "lease_expires_at": row.get::<chrono::DateTime<chrono::Utc>,_>("lease_expires_at")
    })))
}

async fn complete_step(
    State(state): State<AppState>,
    Path(step_id): Path<Uuid>,
    Json(input): Json<CompleteStep>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !matches!(input.status.as_str(), "succeeded" | "failed" | "rejected") {
        return Err((
            StatusCode::BAD_REQUEST,
            "status must be succeeded, failed, or rejected".into(),
        ));
    }

    let mut tx = state.db.begin().await.map_err(internal)?;
    let step = sqlx::query(
        r#"
        SELECT id,orchestration_id,seq,role,state,assigned_principal_id,
               attempt,max_attempts,input_json,output_json,claimed_at,
               lease_expires_at,completed_at,created_at
        FROM orchestration_steps WHERE id=$1 FOR UPDATE
        "#,
    )
    .bind(step_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "orchestration step not found".into()))?;

    if step.get::<Option<Uuid>, _>("assigned_principal_id") != Some(input.principal_id) {
        return Err((
            StatusCode::FORBIDDEN,
            "step is not assigned to this principal".into(),
        ));
    }

    let role = step.get::<String, _>("role");
    ensure_role_in_tx(&mut tx, input.principal_id, &role).await?;
    ensure_capability_in_tx(&mut tx, input.principal_id, capability_for_role(&role)?).await?;

    let current_state = step.get::<String, _>("state");
    if matches!(current_state.as_str(), "completed" | "failed" | "rejected") {
        tx.commit().await.map_err(internal)?;
        return Ok(Json(serde_json::json!({
            "step_id": step_id, "state": current_state, "idempotent": true
        })));
    }

    if current_state != "running" {
        return Err((
            StatusCode::CONFLICT,
            format!("step cannot complete from state {current_state}"),
        ));
    }

    if step
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("lease_expires_at")
        .is_some_and(|expires| expires <= chrono::Utc::now())
    {
        return Err((StatusCode::CONFLICT, "step lease expired".into()));
    }

    if input.status == "rejected" && role != "verifier" {
        return Err((
            StatusCode::FORBIDDEN,
            "only verifier may reject an orchestration".into(),
        ));
    }

    let orchestration_id = step.get::<Uuid, _>("orchestration_id");
    let seq = step.get::<i32, _>("seq");
    let attempt = step.get::<i32, _>("attempt");
    let max_attempts = step.get::<i32, _>("max_attempts");
    let output_json = input.output.to_string();

    if input.status == "failed" && attempt < max_attempts {
        sqlx::query(
            r#"
            UPDATE orchestration_steps
            SET state='pending',assigned_principal_id=NULL,output_json=$1,
                claimed_at=NULL,lease_expires_at=NULL
            WHERE id=$2
            "#,
        )
        .bind(&output_json)
        .bind(step_id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;

        append_event_tx(
            &mut tx,
            orchestration_id,
            Some(step_id),
            Some(input.principal_id),
            "orchestration.step.retry",
            "agent reported failure; bounded retry scheduled",
        )
        .await?;

        tx.commit().await.map_err(internal)?;
        return Ok(Json(serde_json::json!({
            "step_id": step_id, "state": "pending", "orchestration_state": "running",
            "retry": true, "attempt": attempt, "max_attempts": max_attempts
        })));
    }

    let step_state = match input.status.as_str() {
        "succeeded" => "completed",
        "rejected" => "rejected",
        _ => "failed",
    };

    sqlx::query(
        "UPDATE orchestration_steps SET state=$1,output_json=$2,completed_at=now(),lease_expires_at=NULL WHERE id=$3",
    )
    .bind(step_state)
    .bind(&output_json)
    .bind(step_id)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    if input.status == "failed" {
        sqlx::query("UPDATE orchestrations SET state='failed',updated_at=now() WHERE id=$1")
            .bind(orchestration_id)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        append_event_tx(
            &mut tx,
            orchestration_id,
            Some(step_id),
            Some(input.principal_id),
            "orchestration.failed",
            "step exhausted bounded retries",
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        return Ok(Json(serde_json::json!({
            "step_id": step_id, "state": "failed", "orchestration_state": "failed", "retry": false
        })));
    }

    if input.status == "rejected" {
        sqlx::query("UPDATE orchestrations SET state='rejected',updated_at=now() WHERE id=$1")
            .bind(orchestration_id)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        append_event_tx(
            &mut tx,
            orchestration_id,
            Some(step_id),
            Some(input.principal_id),
            "orchestration.rejected",
            "independent verifier rejected final output",
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        return Ok(Json(serde_json::json!({
            "step_id": step_id, "state": "rejected", "orchestration_state": "rejected"
        })));
    }

    if role == "verifier" {
        sqlx::query("UPDATE orchestrations SET state='verified',updated_at=now() WHERE id=$1")
            .bind(orchestration_id)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        append_event_tx(
            &mut tx,
            orchestration_id,
            Some(step_id),
            Some(input.principal_id),
            "orchestration.verified",
            "independent verifier accepted final output",
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        return Ok(Json(serde_json::json!({
            "step_id": step_id, "state": "completed", "orchestration_state": "verified"
        })));
    }

    let next =
        sqlx::query("SELECT id FROM orchestration_steps WHERE orchestration_id=$1 AND seq=$2")
            .bind(orchestration_id)
            .bind(seq + 1)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?;
    let next_step_id = next.get::<Uuid, _>("id");

    sqlx::query("UPDATE orchestration_steps SET input_json=$1 WHERE id=$2 AND state='pending'")
        .bind(&output_json)
        .bind(next_step_id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;

    sqlx::query(
        "UPDATE orchestrations SET current_seq=$1,updated_at=now() WHERE id=$2 AND state='running'",
    )
    .bind(seq + 1)
    .bind(orchestration_id)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    sqlx::query(
        r#"
        INSERT INTO orchestration_handoffs
          (id,orchestration_id,from_step_id,to_step_id,from_principal_id,payload_json,created_at)
        VALUES ($1,$2,$3,$4,$5,$6,now())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(orchestration_id)
    .bind(step_id)
    .bind(next_step_id)
    .bind(input.principal_id)
    .bind(&output_json)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    append_event_tx(
        &mut tx,
        orchestration_id,
        Some(step_id),
        Some(input.principal_id),
        "orchestration.handoff.created",
        "agent completed step and passed durable output to next role",
    )
    .await?;

    tx.commit().await.map_err(internal)?;

    Ok(Json(serde_json::json!({
        "step_id": step_id, "state": "completed", "orchestration_state": "running",
        "next_seq": seq + 1, "next_step_id": next_step_id
    })))
}

async fn active_agent_role(
    db: &PgPool,
    principal_id: Uuid,
) -> Result<String, (StatusCode, String)> {
    let row = sqlx::query(
        r#"
        SELECT a.role FROM principals p
        JOIN agents a ON a.principal_id=p.id
        WHERE p.id=$1 AND p.kind='agent' AND p.status='active'
        "#,
    )
    .bind(principal_id)
    .fetch_optional(db)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::FORBIDDEN, "active agent required".into()))?;
    Ok(row.get::<String, _>("role"))
}

fn capability_for_role(role: &str) -> Result<&'static str, (StatusCode, String)> {
    match role {
        "planner" => Ok("plan.create"),
        "researcher" => Ok("web.research"),
        "worker" => Ok("task.execute"),
        "verifier" => Ok("outcome.verify"),
        _ => Err((
            StatusCode::FORBIDDEN,
            format!("unsupported orchestration role {role}"),
        )),
    }
}

async fn ensure_role_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal_id: Uuid,
    expected: &str,
) -> Result<(), (StatusCode, String)> {
    let row = sqlx::query(
        r#"
        SELECT 1 FROM principals p JOIN agents a ON a.principal_id=p.id
        WHERE p.id=$1 AND p.kind='agent' AND p.status='active' AND a.role=$2
        "#,
    )
    .bind(principal_id)
    .bind(expected)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    if row.is_none() {
        return Err((
            StatusCode::FORBIDDEN,
            format!("active {expected} agent required"),
        ));
    }
    Ok(())
}

async fn ensure_capability_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal_id: Uuid,
    capability: &str,
) -> Result<(), (StatusCode, String)> {
    let row = sqlx::query(
        r#"
        SELECT 1 FROM principal_capabilities pc
        JOIN capabilities c ON c.id=pc.capability_id
        WHERE pc.principal_id=$1 AND c.name=$2
        "#,
    )
    .bind(principal_id)
    .bind(capability)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    if row.is_none() {
        return Err((
            StatusCode::FORBIDDEN,
            format!("principal lacks capability {capability}"),
        ));
    }
    Ok(())
}

async fn append_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    orchestration_id: Uuid,
    step_id: Option<Uuid>,
    principal_id: Option<Uuid>,
    kind: &str,
    message: &str,
) -> Result<(), (StatusCode, String)> {
    sqlx::query(
        r#"
        INSERT INTO orchestration_events
          (id,orchestration_id,step_id,principal_id,kind,message,created_at)
        VALUES ($1,$2,$3,$4,$5,$6,now())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(orchestration_id)
    .bind(step_id)
    .bind(principal_id)
    .bind(kind)
    .bind(message)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    Ok(())
}

fn step_json(row: sqlx::postgres::PgRow) -> serde_json::Value {
    serde_json::json!({
        "id": row.get::<Uuid,_>("id"),
        "orchestration_id": row.get::<Uuid,_>("orchestration_id"),
        "seq": row.get::<i32,_>("seq"),
        "role": row.get::<String,_>("role"),
        "state": row.get::<String,_>("state"),
        "assigned_principal_id": row.get::<Option<Uuid>,_>("assigned_principal_id"),
        "attempt": row.get::<i32,_>("attempt"),
        "max_attempts": row.get::<i32,_>("max_attempts"),
        "input": parse_json(&row.get::<String,_>("input_json")),
        "output": row.get::<Option<String>,_>("output_json").map(|s| parse_json(&s)),
        "claimed_at": row.get::<Option<chrono::DateTime<chrono::Utc>>,_>("claimed_at"),
        "lease_expires_at": row.get::<Option<chrono::DateTime<chrono::Utc>>,_>("lease_expires_at"),
        "completed_at": row.get::<Option<chrono::DateTime<chrono::Utc>>,_>("completed_at"),
        "created_at": row.get::<chrono::DateTime<chrono::Utc>,_>("created_at")
    })
}

fn parse_json(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::json!({"raw": raw}))
}

async fn reconcile_once(db: &PgPool) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;

    sqlx::query(
        "UPDATE task_leases SET state='expired',released_at=COALESCE(released_at,now()) WHERE state='active' AND expires_at <= now()",
    ).execute(&mut *tx).await?;

    sqlx::query(
        r#"
        UPDATE execution_receipts er
        SET status='orphaned',completed_at=now(),
            output_summary=COALESCE(output_summary,'reconciled after execution lease expiry')
        FROM tasks t
        WHERE er.task_id=t.id AND er.status='accepted' AND t.state='running'
          AND NOT EXISTS (
            SELECT 1 FROM task_leases tl
            WHERE tl.task_id=t.id AND tl.worker_principal_id=er.worker_principal_id
              AND tl.state='active' AND tl.expires_at > now()
          )
        "#,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE tasks t SET state='failed',updated_at=now()
        WHERE t.state='running'
          AND EXISTS (SELECT 1 FROM execution_receipts er WHERE er.task_id=t.id AND er.status='orphaned')
          AND NOT EXISTS (SELECT 1 FROM task_leases tl WHERE tl.task_id=t.id AND tl.state='active' AND tl.expires_at > now())
        "#,
    ).execute(&mut *tx).await?;

    sqlx::query(
        r#"
        INSERT INTO orchestration_events
          (id,orchestration_id,step_id,principal_id,kind,message,created_at)
        SELECT gen_random_uuid(),orchestration_id,id,assigned_principal_id,
               'orchestration.step.lease_expired',
               'step lease expired; reconciliation evaluated retry policy',now()
        FROM orchestration_steps
        WHERE state='running' AND lease_expires_at <= now()
        "#,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE orchestration_handoffs h SET to_principal_id=NULL
        FROM orchestration_steps s
        WHERE h.to_step_id=s.id AND s.state='running'
          AND s.lease_expires_at <= now() AND s.attempt < s.max_attempts
        "#,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE orchestration_steps
        SET state='pending',assigned_principal_id=NULL,claimed_at=NULL,lease_expires_at=NULL
        WHERE state='running' AND lease_expires_at <= now() AND attempt < max_attempts
        "#,
    )
    .execute(&mut *tx)
    .await?;

    let exhausted = sqlx::query(
        r#"
        UPDATE orchestration_steps
        SET state='failed',completed_at=now(),lease_expires_at=NULL
        WHERE state='running' AND lease_expires_at <= now() AND attempt >= max_attempts
        RETURNING orchestration_id
        "#,
    )
    .fetch_all(&mut *tx)
    .await?;

    for row in exhausted {
        let orchestration_id = row.get::<Uuid, _>("orchestration_id");
        sqlx::query(
            "UPDATE orchestrations SET state='failed',updated_at=now() WHERE id=$1 AND state='running'",
        ).bind(orchestration_id).execute(&mut *tx).await?;
    }

    tx.commit().await?;
    Ok(())
}

fn internal<E: std::fmt::Display>(error: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}
