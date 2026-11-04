use crate::AppState;
use alp_core::{
    identity::{AgentDefinition, ModelRoutingStrategy, Principal, PrincipalKind},
    project::AgentRuntime,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/principals",
            post(create_principal).get(list_principals),
        )
        .route("/v1/agents", post(create_agent).get(list_agents))
        .route("/v1/clients", post(create_client).get(list_clients))
        .route(
            "/v1/capabilities",
            post(create_capability).get(list_capabilities),
        )
        .route(
            "/v1/principals/{principal_id}/capabilities/{capability_id}",
            post(grant_capability),
        )
}

#[derive(Debug, Deserialize)]
struct CreatePrincipal {
    name: String,
    kind: PrincipalKind,
}

#[derive(Debug, Deserialize)]
struct CreateAgent {
    name: String,
    role: String,
    runtime: AgentRuntime,
    model_provider: Option<String>,
    model_name: Option<String>,
    routing_strategy: ModelRoutingStrategy,
    monthly_budget_eur: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct CreateClient {
    name: String,
    client_type: String,
}

#[derive(Debug, Deserialize)]
struct CreateCapability {
    name: String,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GrantCapability {
    granted_by: String,
}

async fn create_principal(
    State(state): State<AppState>,
    Json(input): Json<CreatePrincipal>,
) -> ApiResult {
    let p = Principal::new(input.name, input.kind);

    sqlx::query(
        "INSERT INTO principals (id,name,kind,status,created_at)
         VALUES ($1,$2,$3,'active',$4)",
    )
    .bind(p.id)
    .bind(&p.name)
    .bind(principal_kind(p.kind))
    .bind(p.created_at)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    registry_event(&state, "principal", p.id, "principal.created").await?;

    Ok(Json(serde_json::json!({
        "id": p.id,
        "name": p.name,
        "kind": principal_kind(p.kind),
        "status": "active"
    })))
}

async fn list_principals(State(state): State<AppState>) -> ApiResult {
    let rows = sqlx::query(
        "SELECT id,name,kind,status,created_at
         FROM principals ORDER BY created_at",
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(serde_json::json!(rows
        .into_iter()
        .map(|r| serde_json::json!({
            "id": r.get::<Uuid,_>("id"),
            "name": r.get::<String,_>("name"),
            "kind": r.get::<String,_>("kind"),
            "status": r.get::<String,_>("status")
        }))
        .collect::<Vec<_>>())))
}

async fn create_agent(State(state): State<AppState>, Json(input): Json<CreateAgent>) -> ApiResult {
    let p = Principal::new(input.name, PrincipalKind::Agent);

    let agent = AgentDefinition {
        principal_id: p.id,
        role: input.role,
        runtime: input.runtime,
        model_provider: input.model_provider,
        model_name: input.model_name,
        routing_strategy: input.routing_strategy,
        monthly_budget_eur: input.monthly_budget_eur,
    };

    agent.validate().map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let mut tx = state.db.begin().await.map_err(internal)?;

    sqlx::query(
        "INSERT INTO principals (id,name,kind,status,created_at)
         VALUES ($1,$2,'agent','active',$3)",
    )
    .bind(p.id)
    .bind(&p.name)
    .bind(p.created_at)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    sqlx::query(
        "INSERT INTO agents
         (principal_id,role,runtime,model_provider,model_name,
          routing_strategy,monthly_budget_eur)
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(p.id)
    .bind(&agent.role)
    .bind(runtime_name(agent.runtime))
    .bind(&agent.model_provider)
    .bind(&agent.model_name)
    .bind(routing_name(agent.routing_strategy))
    .bind(agent.monthly_budget_eur)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    sqlx::query(
        "INSERT INTO registry_events
         (id,subject_type,subject_id,kind,message)
         VALUES ($1,'agent',$2,'agent.created','agent registered')",
    )
    .bind(Uuid::new_v4())
    .bind(p.id)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    tx.commit().await.map_err(internal)?;

    Ok(Json(serde_json::json!({
        "id": p.id,
        "name": p.name,
        "role": agent.role,
        "runtime": runtime_name(agent.runtime),
        "routing_strategy": routing_name(agent.routing_strategy),
        "monthly_budget_eur": agent.monthly_budget_eur
    })))
}

async fn list_agents(State(state): State<AppState>) -> ApiResult {
    let rows = sqlx::query(
        "SELECT p.id,p.name,p.status,a.role,a.runtime,
                a.model_provider,a.model_name,
                a.routing_strategy,a.monthly_budget_eur
         FROM agents a
         JOIN principals p ON p.id=a.principal_id
         ORDER BY p.created_at",
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(serde_json::json!(rows
        .into_iter()
        .map(|r| serde_json::json!({
            "id": r.get::<Uuid,_>("id"),
            "name": r.get::<String,_>("name"),
            "status": r.get::<String,_>("status"),
            "role": r.get::<String,_>("role"),
            "runtime": r.get::<String,_>("runtime"),
            "model_provider": r.get::<Option<String>,_>("model_provider"),
            "model_name": r.get::<Option<String>,_>("model_name"),
            "routing_strategy": r.get::<String,_>("routing_strategy"),
            "monthly_budget_eur": r.get::<Option<f64>,_>("monthly_budget_eur")
        }))
        .collect::<Vec<_>>())))
}

async fn create_client(
    State(state): State<AppState>,
    Json(input): Json<CreateClient>,
) -> ApiResult {
    let p = Principal::new(input.name, PrincipalKind::Client);

    let mut tx = state.db.begin().await.map_err(internal)?;

    sqlx::query(
        "INSERT INTO principals (id,name,kind,status,created_at)
         VALUES ($1,$2,'client','active',$3)",
    )
    .bind(p.id)
    .bind(&p.name)
    .bind(p.created_at)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    sqlx::query(
        "INSERT INTO clients (principal_id,client_type)
         VALUES ($1,$2)",
    )
    .bind(p.id)
    .bind(&input.client_type)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    tx.commit().await.map_err(internal)?;

    Ok(Json(serde_json::json!({
        "id": p.id,
        "name": p.name,
        "client_type": input.client_type
    })))
}

async fn list_clients(State(state): State<AppState>) -> ApiResult {
    let rows = sqlx::query(
        "SELECT p.id,p.name,p.status,c.client_type
         FROM clients c
         JOIN principals p ON p.id=c.principal_id
         ORDER BY p.created_at",
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(serde_json::json!(rows
        .into_iter()
        .map(|r| serde_json::json!({
            "id": r.get::<Uuid,_>("id"),
            "name": r.get::<String,_>("name"),
            "status": r.get::<String,_>("status"),
            "client_type": r.get::<String,_>("client_type")
        }))
        .collect::<Vec<_>>())))
}

async fn create_capability(
    State(state): State<AppState>,
    Json(input): Json<CreateCapability>,
) -> ApiResult {
    let id = Uuid::new_v4();
    let description = input.description.unwrap_or_default();

    sqlx::query(
        "INSERT INTO capabilities (id,name,description)
         VALUES ($1,$2,$3)",
    )
    .bind(id)
    .bind(&input.name)
    .bind(&description)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(serde_json::json!({
        "id": id,
        "name": input.name,
        "description": description
    })))
}

async fn list_capabilities(State(state): State<AppState>) -> ApiResult {
    let rows = sqlx::query(
        "SELECT id,name,description
         FROM capabilities ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(serde_json::json!(rows
        .into_iter()
        .map(|r| serde_json::json!({
            "id": r.get::<Uuid,_>("id"),
            "name": r.get::<String,_>("name"),
            "description": r.get::<String,_>("description")
        }))
        .collect::<Vec<_>>())))
}

async fn grant_capability(
    State(state): State<AppState>,
    Path((principal_id, capability_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<GrantCapability>,
) -> ApiResult {
    sqlx::query(
        "INSERT INTO principal_capabilities
         (principal_id,capability_id,granted_by)
         VALUES ($1,$2,$3)
         ON CONFLICT (principal_id,capability_id)
         DO UPDATE SET granted_by=EXCLUDED.granted_by",
    )
    .bind(principal_id)
    .bind(capability_id)
    .bind(&input.granted_by)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(serde_json::json!({
        "principal_id": principal_id,
        "capability_id": capability_id,
        "granted_by": input.granted_by,
        "state": "granted"
    })))
}

type ApiResult = Result<Json<serde_json::Value>, (StatusCode, String)>;

async fn registry_event(
    state: &AppState,
    subject_type: &str,
    subject_id: Uuid,
    kind: &str,
) -> Result<(), (StatusCode, String)> {
    sqlx::query(
        "INSERT INTO registry_events
         (id,subject_type,subject_id,kind,message)
         VALUES ($1,$2,$3,$4,$4)",
    )
    .bind(Uuid::new_v4())
    .bind(subject_type)
    .bind(subject_id)
    .bind(kind)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    Ok(())
}

fn principal_kind(v: PrincipalKind) -> &'static str {
    match v {
        PrincipalKind::User => "user",
        PrincipalKind::Agent => "agent",
        PrincipalKind::Client => "client",
        PrincipalKind::Service => "service",
        PrincipalKind::System => "system",
    }
}

fn runtime_name(v: AgentRuntime) -> &'static str {
    match v {
        AgentRuntime::Local => "local",
        AgentRuntime::Beam => "beam",
        AgentRuntime::Docker => "docker",
        AgentRuntime::Kubernetes => "kubernetes",
        AgentRuntime::Kata => "kata",
        AgentRuntime::Firecracker => "firecracker",
    }
}

fn routing_name(v: ModelRoutingStrategy) -> &'static str {
    match v {
        ModelRoutingStrategy::LowestCost => "lowest_cost",
        ModelRoutingStrategy::Balanced => "balanced",
        ModelRoutingStrategy::HighestQuality => "highest_quality",
        ModelRoutingStrategy::LowestLatency => "lowest_latency",
        ModelRoutingStrategy::LocalOnly => "local_only",
        ModelRoutingStrategy::Manual => "manual",
    }
}

fn internal<E: std::fmt::Display>(error: E) -> (StatusCode, String) {
    crate::security::internal_error(error)
}
