mod cognition;
mod company;
mod economic;
mod execution;
mod integration;
mod orchestration;
mod registry;
mod security;
mod tools;

use alp_core::{RiskLevel, RuntimeKind, Task, TaskState};
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use std::{env, net::SocketAddr};
use tower_http::trace::TraceLayer;
use tracing::info;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) db: PgPool,
}

#[derive(Debug, Deserialize)]
struct CreateTask {
    name: String,
    description: String,
    risk: RiskLevel,
    runtime: RuntimeKind,
    created_by_principal_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct ApproveTask {
    approved_by_principal_id: Uuid,
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct Health {
    status: &'static str,
    service: &'static str,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "alp_server=info,tower_http=info".into()),
        )
        .init();

    let database_url = match env::var("ALP_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            let password = env::var("ALP_POSTGRES_PASSWORD").map_err(|_| {
                anyhow::anyhow!(
                    "set ALP_DATABASE_URL or ALP_POSTGRES_PASSWORD before starting alp-server"
                )
            })?;
            format!("postgres://alp:{password}@127.0.0.1:5432/alp")
        }
    };

    let bind = env::var("ALP_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    security::validate_startup(&bind)?;

    let db = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;

    migrate(&db).await?;
    orchestration::spawn_reconciler(db.clone());
    company::spawn_reconciler(db.clone());

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/tasks", post(create_task).get(list_tasks))
        .route("/v1/tasks/{id}", get(get_task))
        .route("/v1/tasks/{id}/approve", post(approve_task))
        .merge(execution::router())
        .merge(orchestration::router())
        .merge(tools::router())
        .merge(cognition::router())
        .merge(company::router())
        .merge(integration::router())
        .merge(economic::router())
        .merge(registry::router())
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(axum::middleware::from_fn(security::enforce))
        .layer(TraceLayer::new_for_http())
        .with_state(AppState { db });

    let addr: SocketAddr = bind.parse()?;
    info!(%addr, "ALP control plane listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        service: "alp-control-plane",
    })
}

async fn create_task(
    State(state): State<AppState>,
    Json(input): Json<CreateTask>,
) -> Result<(StatusCode, Json<Task>), (StatusCode, String)> {
    execution::ensure_agent_role(&state.db, input.created_by_principal_id, "planner").await?;
    execution::ensure_capability(&state.db, input.created_by_principal_id, "plan.create").await?;

    let task = Task::new(input.name, input.description, input.risk, input.runtime);

    sqlx::query(
        r#"
        INSERT INTO tasks
          (id, name, description, risk, runtime, state, created_by_principal_id, created_at, updated_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        "#,
    )
    .bind(task.id)
    .bind(&task.name)
    .bind(&task.description)
    .bind(enum_name(task.risk))
    .bind(runtime_name(task.runtime))
    .bind(state_name(task.state))
    .bind(input.created_by_principal_id)
    .bind(task.created_at)
    .bind(task.updated_at)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    execution::append_event_as(
        &state.db,
        Some(task.id),
        Some(input.created_by_principal_id),
        "task.created",
        "planner created task",
    )
    .await?;

    Ok((StatusCode::CREATED, Json(task)))
}

async fn list_tasks(
    State(state): State<AppState>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let rows = sqlx::query(
        r#"
        SELECT id,name,description,risk,runtime,state,created_by_principal_id,created_at,updated_at
        FROM tasks
        ORDER BY created_at DESC
        LIMIT 200
        "#,
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(rows.into_iter().map(row_json).collect()))
}

async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let row = sqlx::query(
        r#"
        SELECT id,name,description,risk,runtime,state,created_by_principal_id,created_at,updated_at
        FROM tasks
        WHERE id=$1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "task not found".into()))?;

    Ok(Json(row_json(row)))
}

async fn approve_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<ApproveTask>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    execution::ensure_principal_kind(&state.db, input.approved_by_principal_id, "user").await?;
    execution::ensure_capability(&state.db, input.approved_by_principal_id, "task.approve").await?;

    let result = sqlx::query(
        r#"
        UPDATE tasks
        SET state='approved', updated_at=now()
        WHERE id=$1 AND state='awaiting_approval'
        "#,
    )
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    if result.rows_affected() != 1 {
        return Err((StatusCode::CONFLICT, "task is not awaiting approval".into()));
    }

    sqlx::query(
        r#"
        INSERT INTO approvals
          (id,task_id,approved_by,reason,created_at)
        VALUES ($1,$2,$3,$4,now())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(id)
    .bind(input.approved_by_principal_id.to_string())
    .bind(&input.reason)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    execution::append_event_as(
        &state.db,
        Some(id),
        Some(input.approved_by_principal_id),
        "task.approved",
        "authorized human approval recorded",
    )
    .await?;

    Ok(Json(serde_json::json!({
        "task_id": id,
        "state": "approved",
        "approved_by_principal_id": input.approved_by_principal_id
    })))
}

async fn migrate(db: &PgPool) -> anyhow::Result<()> {
    let migrations = [
        include_str!("../../../migrations/0001_init.sql"),
        include_str!("../../../migrations/0002_identity_registry.sql"),
        include_str!("../../../migrations/0003_execution_pipeline.sql"),
        include_str!("../../../migrations/0004_orchestration.sql"),
        include_str!("../../../migrations/0005_real_tool_plane.sql"),
        include_str!("../../../migrations/0006_cognitive_provider.sql"),
        include_str!("../../../migrations/0007_company_kernel.sql"),
        include_str!("../../../migrations/0008_sandbox_integration_blueprint.sql"),
        include_str!("../../../migrations/0009_economic_kernel.sql"),
    ];

    for sql in migrations {
        for statement in sql.split(';') {
            let s = statement.trim();

            if !s.is_empty() {
                sqlx::query(s).execute(db).await?;
            }
        }
    }

    Ok(())
}

fn enum_name(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
        RiskLevel::Critical => "critical",
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

fn row_json(row: sqlx::postgres::PgRow) -> serde_json::Value {
    serde_json::json!({
        "id": row.get::<Uuid,_>("id"),
        "name": row.get::<String,_>("name"),
        "description": row.get::<String,_>("description"),
        "risk": row.get::<String,_>("risk"),
        "runtime": row.get::<String,_>("runtime"),
        "state": row.get::<String,_>("state"),
        "created_by_principal_id": row.get::<Option<Uuid>,_>("created_by_principal_id"),
        "created_at": row.get::<chrono::DateTime<chrono::Utc>,_>("created_at"),
        "updated_at": row.get::<chrono::DateTime<chrono::Utc>,_>("updated_at")
    })
}

fn internal<E: std::fmt::Display>(error: E) -> (StatusCode, String) {
    crate::security::internal_error(error)
}
