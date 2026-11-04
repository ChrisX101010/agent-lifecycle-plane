use crate::{execution, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct CreateBlueprint {
    name: String,
    version: i32,
    source_workflow_definition_id: Uuid,
    created_by_principal_id: Uuid,
    #[serde(default)]
    classification: Option<String>,
    #[serde(default)]
    business_context: Value,
    #[serde(default)]
    sensitive: Value,
}

#[derive(Debug, Deserialize)]
struct ImportBlueprint {
    name: String,
    version: i32,
    created_by_principal_id: Uuid,
    package: Value,
    #[serde(default)]
    source_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AddMapping {
    created_by_principal_id: Uuid,
    domain: String,
    canonical_type: String,
    target_system: String,
    target_type: String,
    owner_system: String,
    #[serde(default = "default_direction")]
    direction: String,
    #[serde(default)]
    mapping: Value,
}

#[derive(Debug, Deserialize)]
struct RegisterConnector {
    name: String,
    provider: String,
    transport: String,
    environment: String,
    mode: String,
    #[serde(default)]
    credential_ref: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    config: Value,
    created_by_principal_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct ReplayRequest {
    source_case_id: Uuid,
    target_blueprint_id: Uuid,
    requested_by_principal_id: Uuid,
    #[serde(default)]
    mode: Option<String>,
}

fn default_direction() -> String {
    "bidirectional".into()
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/company/blueprints",
            post(create_blueprint).get(list_blueprints),
        )
        .route("/v1/company/blueprints/import", post(import_blueprint))
        .route("/v1/company/blueprints/{id}", get(get_blueprint))
        .route(
            "/v1/company/blueprints/{id}/export/{profile}",
            get(export_blueprint),
        )
        .route(
            "/v1/company/blueprints/{id}/mappings",
            post(add_mapping).get(list_mappings),
        )
        .route(
            "/v1/company/connectors",
            post(register_connector).get(list_connectors),
        )
        .route("/v1/company/simulations/replay", post(replay_case))
        .route("/v1/company/simulations/{id}", get(get_simulation))
}

type ApiResult = Result<Json<Value>, (StatusCode, String)>;

async fn require_designer(
    db: &sqlx::PgPool,
    principal_id: Uuid,
) -> Result<(), (StatusCode, String)> {
    execution::ensure_principal_kind(db, principal_id, "user").await?;
    execution::ensure_capability(db, principal_id, "task.approve").await?;
    Ok(())
}

async fn create_blueprint(
    State(state): State<AppState>,
    Json(input): Json<CreateBlueprint>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    require_designer(&state.db, input.created_by_principal_id).await?;
    validate_name_version(&input.name, input.version)?;
    let classification = input
        .classification
        .unwrap_or_else(|| "confidential".into());
    validate_classification(&classification)?;

    let wf = sqlx::query(
        "SELECT id,name,version,case_type,state,definition_json FROM workflow_definitions WHERE id=$1",
    )
    .bind(input.source_workflow_definition_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "workflow definition not found".into()))?;
    if wf.get::<String, _>("state") != "published" {
        return Err((
            StatusCode::CONFLICT,
            "workflow definition is not published".into(),
        ));
    }

    let definition = parse_json(&wf.get::<String, _>("definition_json"));
    let package = json!({
        "schema": "alp.company-blueprint/v1",
        "name": input.name.trim(),
        "version": input.version,
        "source": {
            "workflow_definition_id": input.source_workflow_definition_id
        },
        "business": {
            "case_type": wf.get::<String,_>("case_type"),
            "workflow": {
                "name": wf.get::<String,_>("name"),
                "version": wf.get::<i32,_>("version"),
                "definition": definition
            },
            "context": normalized_object(input.business_context)
        },
        "security": {
            "classification": classification.clone(),
            "authorization_binding": "local-only"
        },
        "sensitive": normalized_object(input.sensitive)
    });
    let package_text = package.to_string();
    let hash = sha256_hex(package_text.as_bytes());
    let id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO company_blueprints
           (id,name,version,classification,source_workflow_definition_id,package_json,package_sha256,
            created_by_principal_id,created_at)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,now())"#,
    )
    .bind(id)
    .bind(input.name.trim())
    .bind(input.version)
    .bind(&classification)
    .bind(input.source_workflow_definition_id)
    .bind(&package_text)
    .bind(&hash)
    .bind(input.created_by_principal_id)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "name": input.name.trim(),
            "version": input.version,
            "classification": classification,
            "package_sha256": hash,
            "schema": "alp.company-blueprint/v1"
        })),
    ))
}

async fn import_blueprint(
    State(state): State<AppState>,
    Json(input): Json<ImportBlueprint>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    require_designer(&state.db, input.created_by_principal_id).await?;
    validate_name_version(&input.name, input.version)?;
    if input.package.get("schema").and_then(Value::as_str) != Some("alp.company-blueprint/v1") {
        return Err((
            StatusCode::BAD_REQUEST,
            "unsupported blueprint schema".into(),
        ));
    }
    if input
        .package
        .pointer("/business/workflow/definition/nodes")
        .and_then(Value::as_array)
        .is_none()
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "blueprint has no workflow node array".into(),
        ));
    }

    let source_text = input.package.to_string();
    let source_hash = sha256_hex(source_text.as_bytes());
    if let Some(expected) = input.source_sha256.as_deref() {
        if expected != source_hash {
            return Err((
                StatusCode::BAD_REQUEST,
                "source_sha256 does not match imported package".into(),
            ));
        }
    }

    let mut package = input.package;
    strip_import_authority(&mut package);
    if let Some(obj) = package.as_object_mut() {
        obj.insert("name".into(), Value::String(input.name.trim().into()));
        obj.insert("version".into(), Value::from(input.version));
    }
    let classification = package
        .pointer("/security/classification")
        .and_then(Value::as_str)
        .unwrap_or("confidential")
        .to_string();
    validate_classification(&classification)?;
    let package_text = package.to_string();
    let package_hash = sha256_hex(package_text.as_bytes());
    let id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO company_blueprints
           (id,name,version,classification,source_workflow_definition_id,package_json,package_sha256,
            imported_from_sha256,created_by_principal_id,created_at)
           VALUES ($1,$2,$3,$4,NULL,$5,$6,$7,$8,now())"#,
    )
    .bind(id)
    .bind(input.name.trim())
    .bind(input.version)
    .bind(&classification)
    .bind(&package_text)
    .bind(&package_hash)
    .bind(&source_hash)
    .bind(input.created_by_principal_id)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "name": input.name.trim(),
            "version": input.version,
            "classification": classification,
            "source_sha256": source_hash,
            "package_sha256": package_hash,
            "authorization_binding": "unbound-on-import"
        })),
    ))
}

async fn list_blueprints(State(state): State<AppState>) -> ApiResult {
    let rows = sqlx::query(
        r#"SELECT id,name,version,classification,source_workflow_definition_id,package_sha256,
                  imported_from_sha256,created_by_principal_id,created_at
           FROM company_blueprints ORDER BY name,version DESC"#,
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    Ok(Json(Value::Array(
        rows.into_iter().map(blueprint_summary).collect(),
    )))
}

async fn get_blueprint(State(state): State<AppState>, Path(id): Path<Uuid>) -> ApiResult {
    let row = blueprint_row(&state.db, id).await?;
    let package = parse_json(&row.get::<String, _>("package_json"));
    Ok(Json(json!({
        "id": row.get::<Uuid,_>("id"),
        "name": row.get::<String,_>("name"),
        "version": row.get::<i32,_>("version"),
        "classification": row.get::<String,_>("classification"),
        "package_sha256": row.get::<String,_>("package_sha256"),
        "package": package,
        "created_at": row.get::<DateTime<Utc>,_>("created_at")
    })))
}

async fn export_blueprint(
    State(state): State<AppState>,
    Path((id, profile)): Path<(Uuid, String)>,
) -> ApiResult {
    let row = blueprint_row(&state.db, id).await?;
    let mut package = parse_json(&row.get::<String, _>("package_json"));
    sanitize_export(&mut package, &profile)?;
    let export_text = package.to_string();
    Ok(Json(json!({
        "blueprint_id": id,
        "profile": profile,
        "source_package_sha256": row.get::<String,_>("package_sha256"),
        "export_sha256": sha256_hex(export_text.as_bytes()),
        "package": package
    })))
}

async fn add_mapping(
    State(state): State<AppState>,
    Path(blueprint_id): Path<Uuid>,
    Json(input): Json<AddMapping>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    require_designer(&state.db, input.created_by_principal_id).await?;
    blueprint_row(&state.db, blueprint_id).await?;
    for value in [
        &input.domain,
        &input.canonical_type,
        &input.target_system,
        &input.target_type,
        &input.owner_system,
    ] {
        if value.trim().is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "mapping fields cannot be empty".into(),
            ));
        }
    }
    if !matches!(
        input.direction.as_str(),
        "inbound" | "outbound" | "bidirectional"
    ) {
        return Err((
            StatusCode::BAD_REQUEST,
            "direction must be inbound, outbound, or bidirectional".into(),
        ));
    }
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO blueprint_mappings
           (id,blueprint_id,domain,canonical_type,target_system,target_type,owner_system,direction,mapping_json,
            created_by_principal_id,created_at)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,now())"#,
    )
    .bind(id)
    .bind(blueprint_id)
    .bind(input.domain.trim())
    .bind(input.canonical_type.trim())
    .bind(input.target_system.trim())
    .bind(input.target_type.trim())
    .bind(input.owner_system.trim())
    .bind(&input.direction)
    .bind(normalized_object(input.mapping).to_string())
    .bind(input.created_by_principal_id)
    .execute(&state.db)
    .await
    .map_err(internal)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "blueprint_id": blueprint_id,
            "domain": input.domain.trim(),
            "canonical_type": input.canonical_type.trim(),
            "target_system": input.target_system.trim(),
            "target_type": input.target_type.trim(),
            "owner_system": input.owner_system.trim(),
            "direction": input.direction
        })),
    ))
}

async fn list_mappings(State(state): State<AppState>, Path(blueprint_id): Path<Uuid>) -> ApiResult {
    let rows = sqlx::query(
        r#"SELECT id,domain,canonical_type,target_system,target_type,owner_system,direction,mapping_json,created_at
           FROM blueprint_mappings WHERE blueprint_id=$1 ORDER BY domain,canonical_type,target_system"#,
    )
    .bind(blueprint_id)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    Ok(Json(Value::Array(
        rows.into_iter()
            .map(|r| {
                json!({
                    "id": r.get::<Uuid,_>("id"),
                    "domain": r.get::<String,_>("domain"),
                    "canonical_type": r.get::<String,_>("canonical_type"),
                    "target_system": r.get::<String,_>("target_system"),
                    "target_type": r.get::<String,_>("target_type"),
                    "owner_system": r.get::<String,_>("owner_system"),
                    "direction": r.get::<String,_>("direction"),
                    "mapping": parse_json(&r.get::<String,_>("mapping_json")),
                    "created_at": r.get::<DateTime<Utc>,_>("created_at")
                })
            })
            .collect(),
    )))
}

async fn register_connector(
    State(state): State<AppState>,
    Json(input): Json<RegisterConnector>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    require_designer(&state.db, input.created_by_principal_id).await?;
    for value in [&input.name, &input.provider, &input.transport] {
        if value.trim().is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "connector name/provider/transport cannot be empty".into(),
            ));
        }
    }
    if !matches!(
        input.environment.as_str(),
        "sandbox" | "shadow" | "production"
    ) {
        return Err((
            StatusCode::BAD_REQUEST,
            "environment must be sandbox, shadow, or production".into(),
        ));
    }
    if !matches!(input.mode.as_str(), "simulated" | "read_only" | "live") {
        return Err((
            StatusCode::BAD_REQUEST,
            "mode must be simulated, read_only, or live".into(),
        ));
    }
    if input.environment != "production" && input.mode == "live" {
        return Err((
            StatusCode::FORBIDDEN,
            "live side-effect connectors are forbidden outside production".into(),
        ));
    }
    if contains_secret_material(&input.config) {
        return Err((
            StatusCode::BAD_REQUEST,
            "connector config must not contain inline secrets; use credential_ref".into(),
        ));
    }
    if let Some(reference) = input.credential_ref.as_deref() {
        if !(reference.starts_with("vault://") || reference.starts_with("secret://")) {
            return Err((
                StatusCode::BAD_REQUEST,
                "credential_ref must use vault:// or secret://".into(),
            ));
        }
    }
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO connector_registrations
           (id,name,provider,transport,environment,mode,credential_ref,capabilities_json,config_json,state,
            created_by_principal_id,created_at)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'active',$10,now())"#,
    )
    .bind(id)
    .bind(input.name.trim())
    .bind(input.provider.trim())
    .bind(input.transport.trim())
    .bind(&input.environment)
    .bind(&input.mode)
    .bind(&input.credential_ref)
    .bind(serde_json::to_string(&input.capabilities).unwrap_or_else(|_| "[]".into()))
    .bind(normalized_object(input.config).to_string())
    .bind(input.created_by_principal_id)
    .execute(&state.db)
    .await
    .map_err(internal)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "name": input.name.trim(),
            "provider": input.provider.trim(),
            "transport": input.transport.trim(),
            "environment": input.environment,
            "mode": input.mode,
            "credential_ref": input.credential_ref,
            "state": "active"
        })),
    ))
}

async fn list_connectors(State(state): State<AppState>) -> ApiResult {
    let rows = sqlx::query(
        r#"SELECT id,name,provider,transport,environment,mode,credential_ref,capabilities_json,config_json,state,created_at
           FROM connector_registrations ORDER BY created_at DESC"#,
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    Ok(Json(Value::Array(
        rows.into_iter()
            .map(|r| {
                json!({
                    "id": r.get::<Uuid,_>("id"),
                    "name": r.get::<String,_>("name"),
                    "provider": r.get::<String,_>("provider"),
                    "transport": r.get::<String,_>("transport"),
                    "environment": r.get::<String,_>("environment"),
                    "mode": r.get::<String,_>("mode"),
                    "credential_ref": r.get::<Option<String>,_>("credential_ref"),
                    "capabilities": parse_json(&r.get::<String,_>("capabilities_json")),
                    "config": parse_json(&r.get::<String,_>("config_json")),
                    "state": r.get::<String,_>("state"),
                    "created_at": r.get::<DateTime<Utc>,_>("created_at")
                })
            })
            .collect(),
    )))
}

async fn replay_case(
    State(state): State<AppState>,
    Json(input): Json<ReplayRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    require_designer(&state.db, input.requested_by_principal_id).await?;
    let mode = input.mode.unwrap_or_else(|| "sandbox".into());
    if !matches!(mode.as_str(), "sandbox" | "shadow") {
        return Err((
            StatusCode::BAD_REQUEST,
            "replay mode must be sandbox or shadow".into(),
        ));
    }
    let source_case = sqlx::query(
        "SELECT id,case_type,title,state,workflow_version,environment FROM company_cases WHERE id=$1",
    )
    .bind(input.source_case_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "source case not found".into()))?;
    let blueprint = blueprint_row(&state.db, input.target_blueprint_id).await?;
    let package = parse_json(&blueprint.get::<String, _>("package_json"));
    let target_nodes = package
        .pointer("/business/workflow/definition/nodes")
        .and_then(Value::as_array)
        .ok_or((
            StatusCode::BAD_REQUEST,
            "target blueprint has no workflow nodes".into(),
        ))?;

    let source_rows = sqlx::query(
        "SELECT node_key,kind,role,state FROM case_work_items WHERE case_id=$1 ORDER BY node_key",
    )
    .bind(input.source_case_id)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;

    let mut source = BTreeMap::<String, (String, String)>::new();
    let mut terminal_states = BTreeMap::<String, String>::new();
    for row in source_rows {
        let key = row.get::<String, _>("node_key");
        source.insert(
            key.clone(),
            (row.get::<String, _>("kind"), row.get::<String, _>("role")),
        );
        terminal_states.insert(key, row.get::<String, _>("state"));
    }
    let mut target = BTreeMap::<String, (String, String)>::new();
    for node in target_nodes {
        let key = node
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if key.is_empty() {
            continue;
        }
        let kind = node
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("task")
            .to_string();
        let role = node
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        target.insert(key, (kind, role));
    }

    let source_keys: BTreeSet<_> = source.keys().cloned().collect();
    let target_keys: BTreeSet<_> = target.keys().cloned().collect();
    let missing_in_target: Vec<_> = source_keys.difference(&target_keys).cloned().collect();
    let missing_in_source: Vec<_> = target_keys.difference(&source_keys).cloned().collect();
    let mut changed = Vec::<Value>::new();
    let mut exact = 0usize;
    for key in source_keys.intersection(&target_keys) {
        if source.get(key) == target.get(key) {
            exact += 1;
        } else {
            changed.push(json!({"node":key,"source":source.get(key),"target":target.get(key)}));
        }
    }
    let denominator = source.len().max(target.len()).max(1);
    let equivalence = (exact as f64) * 100.0 / (denominator as f64);
    let source_terminal = terminal_states
        .values()
        .all(|s| matches!(s.as_str(), "completed" | "skipped"));
    let result = json!({
        "source_case": {
            "id": input.source_case_id,
            "case_type": source_case.get::<String,_>("case_type"),
            "title": source_case.get::<String,_>("title"),
            "state": source_case.get::<String,_>("state"),
            "workflow_version": source_case.get::<i32,_>("workflow_version"),
            "environment": source_case.get::<String,_>("environment")
        },
        "target_blueprint_id": input.target_blueprint_id,
        "target_blueprint_sha256": blueprint.get::<String,_>("package_sha256"),
        "mode": mode.clone(),
        "source_terminal": source_terminal,
        "source_nodes": source.len(),
        "target_nodes": target.len(),
        "exact_nodes": exact,
        "process_equivalence": equivalence,
        "diff": {
            "missing_in_target": missing_in_target,
            "missing_in_source": missing_in_source,
            "changed": changed
        },
        "side_effects_executed": false,
        "authority": "simulation-only"
    });
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO simulation_runs
           (id,source_case_id,target_blueprint_id,mode,state,process_equivalence,side_effects_executed,result_json,
            requested_by_principal_id,created_at,completed_at)
           VALUES ($1,$2,$3,$4,'completed',$5,FALSE,$6,$7,now(),now())"#,
    )
    .bind(id)
    .bind(input.source_case_id)
    .bind(input.target_blueprint_id)
    .bind(&mode)
    .bind(equivalence)
    .bind(result.to_string())
    .bind(input.requested_by_principal_id)
    .execute(&state.db)
    .await
    .map_err(internal)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id":id,"state":"completed","result":result})),
    ))
}

async fn get_simulation(State(state): State<AppState>, Path(id): Path<Uuid>) -> ApiResult {
    let row = sqlx::query(
        r#"SELECT id,source_case_id,target_blueprint_id,mode,state,process_equivalence,side_effects_executed,
                  result_json,requested_by_principal_id,created_at,completed_at
           FROM simulation_runs WHERE id=$1"#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "simulation not found".into()))?;
    Ok(Json(json!({
        "id": row.get::<Uuid,_>("id"),
        "source_case_id": row.get::<Uuid,_>("source_case_id"),
        "target_blueprint_id": row.get::<Uuid,_>("target_blueprint_id"),
        "mode": row.get::<String,_>("mode"),
        "state": row.get::<String,_>("state"),
        "process_equivalence": row.get::<f64,_>("process_equivalence"),
        "side_effects_executed": row.get::<bool,_>("side_effects_executed"),
        "result": parse_json(&row.get::<String,_>("result_json")),
        "requested_by_principal_id": row.get::<Uuid,_>("requested_by_principal_id"),
        "created_at": row.get::<DateTime<Utc>,_>("created_at"),
        "completed_at": row.get::<Option<DateTime<Utc>>,_>("completed_at")
    })))
}

async fn blueprint_row(
    db: &sqlx::PgPool,
    id: Uuid,
) -> Result<sqlx::postgres::PgRow, (StatusCode, String)> {
    sqlx::query(
        r#"SELECT id,name,version,classification,source_workflow_definition_id,package_json,package_sha256,
                  imported_from_sha256,created_by_principal_id,created_at
           FROM company_blueprints WHERE id=$1"#,
    )
    .bind(id)
    .fetch_optional(db)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "blueprint not found".into()))
}

fn blueprint_summary(r: sqlx::postgres::PgRow) -> Value {
    json!({
        "id": r.get::<Uuid,_>("id"),
        "name": r.get::<String,_>("name"),
        "version": r.get::<i32,_>("version"),
        "classification": r.get::<String,_>("classification"),
        "source_workflow_definition_id": r.get::<Option<Uuid>,_>("source_workflow_definition_id"),
        "package_sha256": r.get::<String,_>("package_sha256"),
        "imported_from_sha256": r.get::<Option<String>,_>("imported_from_sha256"),
        "created_by_principal_id": r.get::<Uuid,_>("created_by_principal_id"),
        "created_at": r.get::<DateTime<Utc>,_>("created_at")
    })
}

fn sanitize_export(package: &mut Value, profile: &str) -> Result<(), (StatusCode, String)> {
    match profile {
        "full" | "internal" => {}
        "partner" => {
            remove_key(package, "sensitive");
            set_export_security(package, "partner");
        }
        "redacted" => {
            remove_key(package, "sensitive");
            if let Some(business) = package.get_mut("business").and_then(Value::as_object_mut) {
                business.insert("context".into(), json!({}));
            }
            set_export_security(package, "redacted");
        }
        "public_template" => {
            remove_key(package, "sensitive");
            remove_key(package, "source");
            if let Some(business) = package.get_mut("business").and_then(Value::as_object_mut) {
                business.insert("context".into(), json!({}));
            }
            set_export_security(package, "public_template");
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "profile must be full, internal, partner, redacted, or public_template".into(),
            ))
        }
    }
    Ok(())
}

fn set_export_security(package: &mut Value, profile: &str) {
    let root = package.as_object_mut().expect("blueprint package object");
    let sec = root.entry("security").or_insert_with(|| json!({}));
    if let Some(sec) = sec.as_object_mut() {
        sec.insert("export_profile".into(), Value::String(profile.into()));
        sec.insert("authorization_binding".into(), Value::String("none".into()));
    }
}

fn strip_import_authority(package: &mut Value) {
    if let Some(root) = package.as_object_mut() {
        for key in ["authority", "credentials", "secrets"] {
            root.remove(key);
        }
        let sec = root.entry("security").or_insert_with(|| json!({}));
        if let Some(sec) = sec.as_object_mut() {
            sec.insert("imported".into(), Value::Bool(true));
            sec.insert(
                "authorization_binding".into(),
                Value::String("unbound-on-import".into()),
            );
        }
    }
}

fn remove_key(value: &mut Value, key: &str) {
    if let Some(obj) = value.as_object_mut() {
        obj.remove(key);
    }
}

fn contains_secret_material(value: &Value) -> bool {
    const SECRET_KEYS: [&str; 8] = [
        "password",
        "secret",
        "token",
        "api_key",
        "apikey",
        "private_key",
        "client_secret",
        "credential",
    ];
    match value {
        Value::Object(map) => map.iter().any(|(k, v)| {
            let key = k.to_ascii_lowercase();
            SECRET_KEYS.iter().any(|needle| key.contains(needle)) || contains_secret_material(v)
        }),
        Value::Array(values) => values.iter().any(contains_secret_material),
        _ => false,
    }
}

fn normalized_object(value: Value) -> Value {
    if value.is_object() {
        value
    } else {
        Value::Object(Map::new())
    }
}

fn validate_name_version(name: &str, version: i32) -> Result<(), (StatusCode, String)> {
    if name.trim().is_empty() || version < 1 {
        return Err((
            StatusCode::BAD_REQUEST,
            "name must be non-empty and version >= 1".into(),
        ));
    }
    Ok(())
}

fn validate_classification(value: &str) -> Result<(), (StatusCode, String)> {
    if !matches!(
        value,
        "public" | "internal" | "confidential" | "restricted" | "secret"
    ) {
        return Err((StatusCode::BAD_REQUEST, "invalid classification".into()));
    }
    Ok(())
}

fn parse_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| json!({}))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn internal(error: impl std::fmt::Display) -> (StatusCode, String) {
    crate::security::internal_error(error)
}
