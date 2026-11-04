use axum::{
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::env;

pub(crate) fn validate_startup(bind: &str) -> anyhow::Result<()> {
    let mode = env::var("ALP_SECURITY_MODE").unwrap_or_else(|_| "local".into());
    let trusted = env::var("ALP_TRUST_CONTAINER_BOUNDARY").ok().as_deref() == Some("1");
    let loopback = bind.starts_with("127.0.0.1:")
        || bind.starts_with("[::1]:")
        || bind.starts_with("localhost:");

    match mode.as_str() {
        "local" => {
            if !loopback && !trusted {
                anyhow::bail!("ALP_SECURITY_MODE=local requires loopback bind or ALP_TRUST_CONTAINER_BOUNDARY=1");
            }
        }
        "token" => {
            let t = env::var("ALP_API_TOKEN").unwrap_or_default();
            if t.len() < 32 {
                anyhow::bail!(
                    "ALP_SECURITY_MODE=token requires ALP_API_TOKEN of at least 32 characters"
                );
            }
            if !loopback && !trusted {
                anyhow::bail!("ALP_SECURITY_MODE=token requires loopback bind or an explicitly trusted container boundary; public exposure requires a separate hardened TLS/reverse-proxy boundary");
            }
        }
        other => anyhow::bail!("unsupported ALP_SECURITY_MODE={other}; use local or token"),
    }
    Ok(())
}

pub(crate) async fn enforce(req: Request, next: Next) -> Response {
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }
    let mode = env::var("ALP_SECURITY_MODE").unwrap_or_else(|_| "local".into());
    if mode == "local" {
        return next.run(req).await;
    }
    if mode != "token" {
        return (StatusCode::SERVICE_UNAVAILABLE, "invalid security mode").into_response();
    }
    let expected = env::var("ALP_API_TOKEN").unwrap_or_default();
    let supplied = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !constant_time_eq(supplied.as_bytes(), expected.as_bytes()) {
        return (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response();
    }
    next.run(req).await
}

pub(crate) fn internal_error(error: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::error!(error = %error, "internal request failure");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal server error".into(),
    )
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        d |= x ^ y;
    }
    d == 0
}

#[cfg(test)]
mod tests {
    use super::{constant_time_eq, internal_error};
    use axum::http::StatusCode;
    #[test]
    fn exact() {
        assert!(constant_time_eq(
            b"01234567890123456789012345678901",
            b"01234567890123456789012345678901"
        ));
        assert!(!constant_time_eq(
            b"01234567890123456789012345678901",
            b"11234567890123456789012345678901"
        ));
    }
    #[test]
    fn internal_errors_are_redacted() {
        let (status, body) = internal_error("database://user:secret@host/db");
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body, "internal server error");
        assert!(!body.contains("secret"));
    }
}
