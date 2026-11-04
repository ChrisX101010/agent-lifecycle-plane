use crate::{execution, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::{collections::HashSet, env, time::Duration};
use uuid::Uuid;

const MAX_EVIDENCE_BYTES: usize = 262_144;
const MAX_ARTIFACT_EXCERPT_CHARS: usize = 8_000;

#[derive(Debug, Deserialize)]
struct WebFetchInput {
    principal_id: Uuid,
    orchestration_id: Uuid,
    url: String,
}

#[derive(Debug, Deserialize)]
struct BuildArtifactInput {
    principal_id: Uuid,
    orchestration_id: Uuid,
    evidence_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct VerifyArtifactInput {
    principal_id: Uuid,
    orchestration_id: Uuid,
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/tools/web-fetch", post(web_fetch))
        .route("/v1/tools/artifacts/build", post(build_artifact))
        .route("/v1/tools/artifacts/{id}/verify", post(verify_artifact))
}

async fn web_fetch(
    State(state): State<AppState>,
    Json(input): Json<WebFetchInput>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    execution::ensure_agent_role(&state.db, input.principal_id, "researcher").await?;
    execution::ensure_capability(&state.db, input.principal_id, "web.research").await?;
    ensure_running_orchestration(&state.db, input.orchestration_id).await?;

    let parsed = reqwest::Url::parse(input.url.trim())
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid research URL".into()))?;
    validate_research_url(&parsed)?;

    let tool_call_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO tool_calls
          (id,orchestration_id,principal_id,tool_name,request_json,state,started_at)
        VALUES ($1,$2,$3,'web.fetch',$4,'started',now())
        "#,
    )
    .bind(tool_call_id)
    .bind(input.orchestration_id)
    .bind(input.principal_id)
    .bind(serde_json::json!({"url": parsed.as_str()}).to_string())
    .execute(&state.db)
    .await
    .map_err(internal)?;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .user_agent("ALP/0.1 governed-research-agent")
        .build()
        .map_err(internal)?;

    let response = match client.get(parsed.clone()).send().await {
        Ok(response) => response,
        Err(error) => {
            let message = error.to_string();
            record_tool_error(&state.db, tool_call_id, &message).await?;
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("web fetch failed: {message}"),
            ));
        }
    };

    let http_status = response.status();
    if http_status.is_redirection() {
        let message = "redirects are disabled by ALP web policy";
        record_tool_error_status(
            &state.db,
            tool_call_id,
            http_status.as_u16() as i32,
            message,
        )
        .await?;
        return Err((StatusCode::BAD_GATEWAY, message.into()));
    }
    if !http_status.is_success() {
        let message = format!("upstream returned HTTP {}", http_status.as_u16());
        record_tool_error_status(
            &state.db,
            tool_call_id,
            http_status.as_u16() as i32,
            &message,
        )
        .await?;
        return Err((StatusCode::BAD_GATEWAY, message));
    }

    if let Some(length) = response.content_length() {
        if length > MAX_EVIDENCE_BYTES as u64 {
            let message = "research response exceeds ALP evidence size limit";
            record_tool_error_status(
                &state.db,
                tool_call_id,
                http_status.as_u16() as i32,
                message,
            )
            .await?;
            return Err((StatusCode::PAYLOAD_TOO_LARGE, message.into()));
        }
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());

    if let Some(ref ct) = content_type {
        let lower = ct.to_ascii_lowercase();
        if !(lower.starts_with("text/")
            || lower.starts_with("application/json")
            || lower.starts_with("application/xml")
            || lower.starts_with("application/xhtml+xml"))
        {
            let message = format!("content type is not admitted as textual evidence: {ct}");
            record_tool_error_status(
                &state.db,
                tool_call_id,
                http_status.as_u16() as i32,
                &message,
            )
            .await?;
            return Err((StatusCode::UNSUPPORTED_MEDIA_TYPE, message));
        }
    }

    let bytes = response.bytes().await.map_err(internal)?;
    if bytes.len() > MAX_EVIDENCE_BYTES {
        let message = "research response exceeds ALP evidence size limit";
        record_tool_error_status(
            &state.db,
            tool_call_id,
            http_status.as_u16() as i32,
            message,
        )
        .await?;
        return Err((StatusCode::PAYLOAD_TOO_LARGE, message.into()));
    }

    let content = String::from_utf8_lossy(bytes.as_ref()).into_owned();
    let sha256 = sha256_hex(bytes.as_ref());
    let evidence_id = Uuid::new_v4();

    let mut tx = state.db.begin().await.map_err(internal)?;
    sqlx::query(
        r#"
        UPDATE tool_calls
        SET state='succeeded',http_status=$1,response_sha256=$2,completed_at=now()
        WHERE id=$3
        "#,
    )
    .bind(http_status.as_u16() as i32)
    .bind(&sha256)
    .bind(tool_call_id)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    sqlx::query(
        r#"
        INSERT INTO evidence_artifacts
          (id,orchestration_id,tool_call_id,principal_id,source_url,content_type,
           content_sha256,byte_length,content,created_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,now())
        "#,
    )
    .bind(evidence_id)
    .bind(input.orchestration_id)
    .bind(tool_call_id)
    .bind(input.principal_id)
    .bind(parsed.as_str())
    .bind(&content_type)
    .bind(&sha256)
    .bind(bytes.len() as i64)
    .bind(&content)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    tx.commit().await.map_err(internal)?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "tool_call_id": tool_call_id,
            "evidence_id": evidence_id,
            "orchestration_id": input.orchestration_id,
            "source_url": parsed.as_str(),
            "http_status": http_status.as_u16(),
            "content_type": content_type,
            "content_sha256": sha256,
            "byte_length": bytes.len(),
            "title": extract_title(&content),
            "external_research_performed": true
        })),
    ))
}

async fn build_artifact(
    State(state): State<AppState>,
    Json(input): Json<BuildArtifactInput>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    execution::ensure_agent_role(&state.db, input.principal_id, "worker").await?;
    execution::ensure_capability(&state.db, input.principal_id, "task.execute").await?;
    ensure_running_orchestration(&state.db, input.orchestration_id).await?;

    let evidence = sqlx::query(
        r#"
        SELECT id,source_url,content_sha256,byte_length,content
        FROM evidence_artifacts
        WHERE id=$1 AND orchestration_id=$2
        "#,
    )
    .bind(input.evidence_id)
    .bind(input.orchestration_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "evidence artifact not found".into()))?;

    let source_url = evidence.get::<String, _>("source_url");
    let evidence_sha256 = evidence.get::<String, _>("content_sha256");
    let byte_length = evidence.get::<i64, _>("byte_length");
    let content = evidence.get::<String, _>("content");
    let excerpt: String = content.chars().take(MAX_ARTIFACT_EXCERPT_CHARS).collect();

    let artifact_content = format!(
        "# ALP Evidence Artifact\n\nOrchestration: `{}`\nEvidence: `{}`\nSource: {}\nSource SHA-256: `{}`\nSource bytes: {}\n\n## Captured evidence excerpt\n\n{}\n",
        input.orchestration_id,
        input.evidence_id,
        source_url,
        evidence_sha256,
        byte_length,
        excerpt
    );

    let artifact_sha256 = sha256_hex(artifact_content.as_bytes());
    let artifact_id = Uuid::new_v4();

    sqlx::query(
        r#"
        INSERT INTO work_artifacts
          (id,orchestration_id,evidence_id,principal_id,media_type,content_sha256,content,created_at)
        VALUES ($1,$2,$3,$4,'text/markdown',$5,$6,now())
        "#,
    )
    .bind(artifact_id)
    .bind(input.orchestration_id)
    .bind(input.evidence_id)
    .bind(input.principal_id)
    .bind(&artifact_sha256)
    .bind(&artifact_content)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "artifact_id": artifact_id,
            "orchestration_id": input.orchestration_id,
            "evidence_id": input.evidence_id,
            "media_type": "text/markdown",
            "content_sha256": artifact_sha256,
            "source_url": source_url,
            "source_sha256": evidence_sha256,
            "artifact_created": true
        })),
    ))
}

async fn verify_artifact(
    State(state): State<AppState>,
    Path(artifact_id): Path<Uuid>,
    Json(input): Json<VerifyArtifactInput>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    execution::ensure_agent_role(&state.db, input.principal_id, "verifier").await?;
    execution::ensure_capability(&state.db, input.principal_id, "outcome.verify").await?;
    ensure_running_orchestration(&state.db, input.orchestration_id).await?;

    let row = sqlx::query(
        r#"
        SELECT wa.evidence_id,wa.principal_id AS worker_principal_id,
               wa.content_sha256,wa.content,
               ea.source_url,ea.content_sha256 AS evidence_sha256,ea.content AS evidence_content
        FROM work_artifacts wa
        JOIN evidence_artifacts ea ON ea.id=wa.evidence_id
        WHERE wa.id=$1 AND wa.orchestration_id=$2 AND ea.orchestration_id=$2
        "#,
    )
    .bind(artifact_id)
    .bind(input.orchestration_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or((StatusCode::NOT_FOUND, "work artifact not found".into()))?;

    let worker_principal_id = row.get::<Uuid, _>("worker_principal_id");
    if worker_principal_id == input.principal_id {
        return Err((
            StatusCode::FORBIDDEN,
            "worker cannot verify its own artifact".into(),
        ));
    }

    let stored_artifact_hash = row.get::<String, _>("content_sha256");
    let artifact_content = row.get::<String, _>("content");
    let recomputed_artifact_hash = sha256_hex(artifact_content.as_bytes());

    let stored_evidence_hash = row.get::<String, _>("evidence_sha256");
    let evidence_content = row.get::<String, _>("evidence_content");
    let recomputed_evidence_hash = sha256_hex(evidence_content.as_bytes());
    let evidence_id = row.get::<Uuid, _>("evidence_id");

    let artifact_hash_ok = stored_artifact_hash == recomputed_artifact_hash;
    let evidence_hash_ok = stored_evidence_hash == recomputed_evidence_hash;
    let provenance_ok = artifact_content.contains(&evidence_id.to_string())
        && artifact_content.contains(&stored_evidence_hash);

    let verified = artifact_hash_ok && evidence_hash_ok && provenance_ok;
    let decision = if verified { "verified" } else { "rejected" };
    let reason = if verified {
        "artifact hash, evidence hash, and evidence provenance link verified"
    } else {
        "artifact/evidence integrity or provenance verification failed"
    };

    sqlx::query(
        r#"
        INSERT INTO artifact_verifications
          (id,orchestration_id,artifact_id,verifier_principal_id,decision,reason,created_at)
        VALUES ($1,$2,$3,$4,$5,$6,now())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(input.orchestration_id)
    .bind(artifact_id)
    .bind(input.principal_id)
    .bind(decision)
    .bind(reason)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    Ok(Json(serde_json::json!({
        "artifact_id": artifact_id,
        "orchestration_id": input.orchestration_id,
        "evidence_id": evidence_id,
        "source_url": row.get::<String,_>("source_url"),
        "artifact_sha256": stored_artifact_hash,
        "evidence_sha256": stored_evidence_hash,
        "artifact_hash_verified": artifact_hash_ok,
        "evidence_hash_verified": evidence_hash_ok,
        "provenance_verified": provenance_ok,
        "verified": verified,
        "decision": decision,
        "reason": reason
    })))
}

async fn ensure_running_orchestration(
    db: &sqlx::PgPool,
    orchestration_id: Uuid,
) -> Result<(), (StatusCode, String)> {
    let row = sqlx::query("SELECT state FROM orchestrations WHERE id=$1")
        .bind(orchestration_id)
        .fetch_optional(db)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "orchestration not found".into()))?;

    if row.get::<String, _>("state") != "running" {
        return Err((
            StatusCode::CONFLICT,
            "tool execution requires a running orchestration".into(),
        ));
    }
    Ok(())
}

fn validate_research_url(url: &reqwest::Url) -> Result<(), (StatusCode, String)> {
    if url.scheme() != "https" {
        return Err((
            StatusCode::FORBIDDEN,
            "ALP web policy permits HTTPS only".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err((
            StatusCode::FORBIDDEN,
            "credentials in research URLs are forbidden".into(),
        ));
    }
    if url.port_or_known_default() != Some(443) {
        return Err((
            StatusCode::FORBIDDEN,
            "non-standard research ports are forbidden".into(),
        ));
    }

    let host = url
        .host_str()
        .ok_or((StatusCode::BAD_REQUEST, "research URL has no host".into()))?
        .trim_end_matches('.')
        .to_ascii_lowercase();

    if host.parse::<std::net::IpAddr>().is_ok()
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
    {
        return Err((
            StatusCode::FORBIDDEN,
            "local/IP research targets are forbidden".into(),
        ));
    }

    let allowlist = env::var("ALP_WEB_ALLOWLIST")
        .unwrap_or_else(|_| "example.com,www.rfc-editor.org,rfc-editor.org".into());
    let allowed: HashSet<String> = allowlist
        .split(',')
        .map(|item| item.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|item| !item.is_empty())
        .collect();

    if !allowed.contains(&host) {
        return Err((
            StatusCode::FORBIDDEN,
            format!("research host {host} is not in ALP_WEB_ALLOWLIST"),
        ));
    }

    Ok(())
}

async fn record_tool_error(
    db: &sqlx::PgPool,
    tool_call_id: Uuid,
    error: &str,
) -> Result<(), (StatusCode, String)> {
    sqlx::query("UPDATE tool_calls SET state='failed',error=$1,completed_at=now() WHERE id=$2")
        .bind(error)
        .bind(tool_call_id)
        .execute(db)
        .await
        .map_err(internal)?;
    Ok(())
}

async fn record_tool_error_status(
    db: &sqlx::PgPool,
    tool_call_id: Uuid,
    http_status: i32,
    error: &str,
) -> Result<(), (StatusCode, String)> {
    sqlx::query(
        "UPDATE tool_calls SET state='failed',http_status=$1,error=$2,completed_at=now() WHERE id=$3",
    )
    .bind(http_status)
    .bind(error)
    .bind(tool_call_id)
    .execute(db)
    .await
    .map_err(internal)?;
    Ok(())
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn extract_title(content: &str) -> Option<String> {
    let lower = content.to_ascii_lowercase();
    let start = lower.find("<title>")? + "<title>".len();
    let rest = &content[start..];
    let rest_lower = &lower[start..];
    let end = rest_lower.find("</title>")?;
    let title = rest[..end].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.chars().take(300).collect())
    }
}

fn internal<E: std::fmt::Display>(error: E) -> (StatusCode, String) {
    crate::security::internal_error(error)
}
