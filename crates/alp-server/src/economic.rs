use crate::{execution, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct CreateAccount {
    legal_entity: String,
    code: String,
    name: String,
    account_type: String,
    currency: String,
    created_by_principal_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct CreateBudget {
    case_id: Uuid,
    amount_minor: i64,
    currency: String,
    #[serde(default)]
    period_start: Option<DateTime<Utc>>,
    #[serde(default)]
    period_end: Option<DateTime<Utc>>,
    created_by_principal_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct CreateCommitment {
    case_id: Uuid,
    description: String,
    #[serde(default)]
    counterparty_ref: Option<String>,
    amount_minor: i64,
    currency: String,
    requested_by_principal_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct ApproveCommitment {
    approved_by_principal_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct CreateInvoice {
    case_id: Uuid,
    direction: String,
    counterparty_ref: String,
    amount_minor: i64,
    currency: String,
    #[serde(default)]
    external_ref: Option<String>,
    created_by_principal_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct ApproveInvoice {
    approved_by_principal_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct CreatePaymentIntent {
    case_id: Uuid,
    #[serde(default)]
    invoice_id: Option<Uuid>,
    direction: String,
    amount_minor: i64,
    currency: String,
    idempotency_key: String,
    requested_by_principal_id: Uuid,
    #[serde(default)]
    metadata: Value,
}

#[derive(Debug, Deserialize)]
struct AuthorizePaymentIntent {
    authorized_by_principal_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct PostJournal {
    case_id: Uuid,
    description: String,
    currency: String,
    #[serde(default)]
    source_type: Option<String>,
    #[serde(default)]
    source_id: Option<Uuid>,
    idempotency_key: String,
    posted_by_principal_id: Uuid,
    lines: Vec<PostJournalLine>,
}

#[derive(Debug, Deserialize)]
struct PostJournalLine {
    account_id: Uuid,
    #[serde(default)]
    debit_minor: i64,
    #[serde(default)]
    credit_minor: i64,
    #[serde(default)]
    memo: String,
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/company/economics/accounts",
            post(create_account).get(list_accounts),
        )
        .route("/v1/company/economics/budgets", post(create_budget))
        .route("/v1/company/economics/commitments", post(create_commitment))
        .route(
            "/v1/company/economics/commitments/{id}/approve",
            post(approve_commitment),
        )
        .route("/v1/company/economics/invoices", post(create_invoice))
        .route(
            "/v1/company/economics/invoices/{id}/approve",
            post(approve_invoice),
        )
        .route(
            "/v1/company/economics/payment-intents",
            post(create_payment_intent),
        )
        .route(
            "/v1/company/economics/payment-intents/{id}/authorize",
            post(authorize_payment_intent),
        )
        .route("/v1/company/economics/journals", post(post_journal))
        .route(
            "/v1/company/economics/cases/{id}/summary",
            get(case_economic_summary),
        )
}

type ApiResult = Result<Json<Value>, (StatusCode, String)>;

async fn require_user_capability(
    db: &PgPool,
    principal_id: Uuid,
    capability: &str,
) -> Result<(), (StatusCode, String)> {
    execution::ensure_principal_kind(db, principal_id, "user").await?;
    execution::ensure_capability(db, principal_id, capability).await?;
    Ok(())
}

async fn require_active_capability(
    db: &PgPool,
    principal_id: Uuid,
    capability: &str,
) -> Result<(), (StatusCode, String)> {
    let active = sqlx::query("SELECT 1 FROM principals WHERE id=$1 AND status='active'")
        .bind(principal_id)
        .fetch_optional(db)
        .await
        .map_err(internal)?
        .is_some();
    if !active {
        return Err((StatusCode::FORBIDDEN, "active principal required".into()));
    }
    execution::ensure_capability(db, principal_id, capability).await?;
    Ok(())
}

async fn create_account(
    State(state): State<AppState>,
    Json(input): Json<CreateAccount>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    require_user_capability(
        &state.db,
        input.created_by_principal_id,
        "finance.configure",
    )
    .await?;
    let account_type = input.account_type.trim().to_lowercase();
    if !matches!(
        account_type.as_str(),
        "asset" | "liability" | "equity" | "revenue" | "expense"
    ) {
        return Err((StatusCode::BAD_REQUEST, "invalid account_type".into()));
    }
    let currency = validate_currency(&input.currency)?;
    if input.legal_entity.trim().is_empty()
        || input.code.trim().is_empty()
        || input.name.trim().is_empty()
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "legal_entity/code/name cannot be empty".into(),
        ));
    }
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO economic_accounts
           (id,legal_entity,code,name,account_type,currency,active,created_by_principal_id,created_at)
           VALUES($1,$2,$3,$4,$5,$6,true,$7,now())"#,
    )
    .bind(id)
    .bind(input.legal_entity.trim())
    .bind(input.code.trim())
    .bind(input.name.trim())
    .bind(&account_type)
    .bind(&currency)
    .bind(input.created_by_principal_id)
    .execute(&state.db)
    .await
    .map_err(conflict_or_internal)?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "legal_entity": input.legal_entity.trim(),
            "code": input.code.trim(),
            "name": input.name.trim(),
            "account_type": account_type,
            "currency": currency,
            "active": true
        })),
    ))
}

async fn list_accounts(State(state): State<AppState>) -> ApiResult {
    let rows = sqlx::query(
        "SELECT id,legal_entity,code,name,account_type,currency,active,created_by_principal_id,created_at FROM economic_accounts ORDER BY legal_entity,code",
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    Ok(Json(Value::Array(
        rows.into_iter()
            .map(|r| {
                json!({
                    "id": r.get::<Uuid,_>("id"),
                    "legal_entity": r.get::<String,_>("legal_entity"),
                    "code": r.get::<String,_>("code"),
                    "name": r.get::<String,_>("name"),
                    "account_type": r.get::<String,_>("account_type"),
                    "currency": r.get::<String,_>("currency"),
                    "active": r.get::<bool,_>("active"),
                    "created_by_principal_id": r.get::<Uuid,_>("created_by_principal_id"),
                    "created_at": r.get::<DateTime<Utc>,_>("created_at")
                })
            })
            .collect(),
    )))
}

async fn create_budget(
    State(state): State<AppState>,
    Json(input): Json<CreateBudget>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    require_user_capability(
        &state.db,
        input.created_by_principal_id,
        "finance.configure",
    )
    .await?;
    ensure_case(&state.db, input.case_id).await?;
    if input.amount_minor <= 0 {
        return Err((StatusCode::BAD_REQUEST, "amount_minor must be > 0".into()));
    }
    if let (Some(start), Some(end)) = (input.period_start.as_ref(), input.period_end.as_ref()) {
        if end <= start {
            return Err((
                StatusCode::BAD_REQUEST,
                "period_end must be after period_start".into(),
            ));
        }
    }
    let currency = validate_currency(&input.currency)?;
    ensure_case_currency_compatible(&state.db, input.case_id, &currency).await?;
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO economic_budgets
           (id,case_id,amount_minor,currency,period_start,period_end,state,created_by_principal_id,created_at)
           VALUES($1,$2,$3,$4,$5,$6,'active',$7,now())"#,
    )
    .bind(id)
    .bind(input.case_id)
    .bind(input.amount_minor)
    .bind(&currency)
    .bind(input.period_start)
    .bind(input.period_end)
    .bind(input.created_by_principal_id)
    .execute(&state.db)
    .await
    .map_err(conflict_or_internal)?;

    append_event(
        &state.db,
        input.case_id,
        Some(input.created_by_principal_id),
        "budget.created",
        "budget",
        Some(id),
        json!({"amount_minor":input.amount_minor,"currency":currency}),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"id":id,"case_id":input.case_id,"amount_minor":input.amount_minor,"currency":currency,"state":"active"}),
        ),
    ))
}

async fn create_commitment(
    State(state): State<AppState>,
    Json(input): Json<CreateCommitment>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    require_active_capability(
        &state.db,
        input.requested_by_principal_id,
        "finance.propose",
    )
    .await?;
    ensure_case(&state.db, input.case_id).await?;
    if input.amount_minor <= 0 || input.description.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "positive amount_minor and description required".into(),
        ));
    }
    let currency = validate_currency(&input.currency)?;
    ensure_budget_headroom(&state.db, input.case_id, &currency, input.amount_minor).await?;
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO economic_commitments
           (id,case_id,description,counterparty_ref,amount_minor,currency,state,requested_by_principal_id,created_at,updated_at)
           VALUES($1,$2,$3,$4,$5,$6,'proposed',$7,now(),now())"#,
    )
    .bind(id).bind(input.case_id).bind(input.description.trim()).bind(input.counterparty_ref.as_deref())
    .bind(input.amount_minor).bind(&currency).bind(input.requested_by_principal_id)
    .execute(&state.db).await.map_err(internal)?;
    append_event(
        &state.db,
        input.case_id,
        Some(input.requested_by_principal_id),
        "commitment.proposed",
        "commitment",
        Some(id),
        json!({"amount_minor":input.amount_minor,"currency":currency}),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"id":id,"case_id":input.case_id,"amount_minor":input.amount_minor,"currency":currency,"state":"proposed"}),
        ),
    ))
}

async fn approve_commitment(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<ApproveCommitment>,
) -> ApiResult {
    require_user_capability(
        &state.db,
        input.approved_by_principal_id,
        "finance.configure",
    )
    .await?;
    let row = sqlx::query(
        r#"UPDATE economic_commitments SET state='approved',approved_by_principal_id=$2,approved_at=now(),updated_at=now()
           WHERE id=$1 AND state='proposed'
           RETURNING case_id,amount_minor,currency"#,
    ).bind(id).bind(input.approved_by_principal_id).fetch_optional(&state.db).await.map_err(internal)?
      .ok_or((StatusCode::CONFLICT,"commitment is not proposed".into()))?;
    let case_id = row.get::<Uuid, _>("case_id");
    append_event(&state.db, case_id, Some(input.approved_by_principal_id), "commitment.approved", "commitment", Some(id), json!({"amount_minor":row.get::<i64,_>("amount_minor"),"currency":row.get::<String,_>("currency")})).await?;
    Ok(Json(
        json!({"id":id,"case_id":case_id,"state":"approved","approved_by_principal_id":input.approved_by_principal_id}),
    ))
}

async fn create_invoice(
    State(state): State<AppState>,
    Json(input): Json<CreateInvoice>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    require_active_capability(&state.db, input.created_by_principal_id, "finance.propose").await?;
    ensure_case(&state.db, input.case_id).await?;
    if input.amount_minor <= 0 || input.counterparty_ref.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "positive amount_minor and counterparty_ref required".into(),
        ));
    }
    let direction = input.direction.trim().to_lowercase();
    if !matches!(direction.as_str(), "receivable" | "payable") {
        return Err((
            StatusCode::BAD_REQUEST,
            "direction must be receivable or payable".into(),
        ));
    }
    let currency = validate_currency(&input.currency)?;
    ensure_case_currency_compatible(&state.db, input.case_id, &currency).await?;
    let state_name = if direction == "receivable" {
        "issued"
    } else {
        "pending_approval"
    };
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO economic_invoices
           (id,case_id,direction,counterparty_ref,amount_minor,currency,state,external_ref,created_by_principal_id,created_at,updated_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,now(),now())"#,
    ).bind(id).bind(input.case_id).bind(&direction).bind(input.counterparty_ref.trim()).bind(input.amount_minor)
      .bind(&currency).bind(state_name).bind(input.external_ref.as_deref()).bind(input.created_by_principal_id)
      .execute(&state.db).await.map_err(internal)?;
    append_event(&state.db,input.case_id,Some(input.created_by_principal_id),"invoice.created","invoice",Some(id),json!({"direction":direction,"amount_minor":input.amount_minor,"currency":currency,"state":state_name})).await?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"id":id,"case_id":input.case_id,"direction":direction,"amount_minor":input.amount_minor,"currency":currency,"state":state_name}),
        ),
    ))
}

async fn approve_invoice(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<ApproveInvoice>,
) -> ApiResult {
    require_user_capability(
        &state.db,
        input.approved_by_principal_id,
        "finance.configure",
    )
    .await?;
    let row=sqlx::query(
        r#"UPDATE economic_invoices SET state='approved',approved_by_principal_id=$2,approved_at=now(),updated_at=now()
           WHERE id=$1 AND direction='payable' AND state='pending_approval'
           RETURNING case_id,amount_minor,currency"#,
    ).bind(id).bind(input.approved_by_principal_id).fetch_optional(&state.db).await.map_err(internal)?
      .ok_or((StatusCode::CONFLICT,"payable invoice is not pending approval".into()))?;
    let case_id = row.get::<Uuid, _>("case_id");
    append_event(&state.db,case_id,Some(input.approved_by_principal_id),"invoice.approved","invoice",Some(id),json!({"amount_minor":row.get::<i64,_>("amount_minor"),"currency":row.get::<String,_>("currency")})).await?;
    Ok(Json(json!({"id":id,"case_id":case_id,"state":"approved"})))
}

async fn create_payment_intent(
    State(state): State<AppState>,
    Json(input): Json<CreatePaymentIntent>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    require_active_capability(
        &state.db,
        input.requested_by_principal_id,
        "finance.propose",
    )
    .await?;
    ensure_case(&state.db, input.case_id).await?;
    if input.amount_minor <= 0 || input.idempotency_key.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "positive amount_minor and idempotency_key required".into(),
        ));
    }
    let direction = input.direction.trim().to_lowercase();
    if !matches!(direction.as_str(), "outbound" | "inbound") {
        return Err((
            StatusCode::BAD_REQUEST,
            "direction must be outbound or inbound".into(),
        ));
    }
    let currency = validate_currency(&input.currency)?;
    ensure_case_currency_compatible(&state.db, input.case_id, &currency).await?;
    if let Some(invoice_id) = input.invoice_id {
        let invoice=sqlx::query("SELECT case_id,direction,amount_minor,currency,state FROM economic_invoices WHERE id=$1")
            .bind(invoice_id).fetch_optional(&state.db).await.map_err(internal)?
            .ok_or((StatusCode::NOT_FOUND,"invoice not found".into()))?;
        if invoice.get::<Uuid, _>("case_id") != input.case_id
            || invoice.get::<String, _>("currency") != currency
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "payment intent does not match invoice case/currency".into(),
            ));
        }
        let inv_direction = invoice.get::<String, _>("direction");
        if direction == "outbound"
            && (inv_direction != "payable" || invoice.get::<String, _>("state") != "approved")
        {
            return Err((
                StatusCode::CONFLICT,
                "outbound invoice payment requires approved payable invoice".into(),
            ));
        }
        if direction == "inbound" && inv_direction != "receivable" {
            return Err((
                StatusCode::CONFLICT,
                "inbound invoice payment requires receivable invoice".into(),
            ));
        }
        if input.amount_minor > invoice.get::<i64, _>("amount_minor") {
            return Err((
                StatusCode::BAD_REQUEST,
                "payment amount exceeds invoice amount".into(),
            ));
        }
    }
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO payment_intents
           (id,case_id,invoice_id,direction,amount_minor,currency,state,idempotency_key,requested_by_principal_id,metadata_json,created_at,updated_at)
           VALUES($1,$2,$3,$4,$5,$6,'proposed',$7,$8,$9,now(),now())"#,
    ).bind(id).bind(input.case_id).bind(input.invoice_id).bind(&direction).bind(input.amount_minor).bind(&currency)
      .bind(input.idempotency_key.trim()).bind(input.requested_by_principal_id).bind(input.metadata.to_string())
      .execute(&state.db).await.map_err(conflict_or_internal)?;
    append_event(&state.db,input.case_id,Some(input.requested_by_principal_id),"payment.proposed","payment_intent",Some(id),json!({"direction":direction,"amount_minor":input.amount_minor,"currency":currency,"executed":false})).await?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"id":id,"case_id":input.case_id,"invoice_id":input.invoice_id,"direction":direction,"amount_minor":input.amount_minor,"currency":currency,"state":"proposed","executed_at":Value::Null}),
        ),
    ))
}

async fn authorize_payment_intent(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<AuthorizePaymentIntent>,
) -> ApiResult {
    require_user_capability(
        &state.db,
        input.authorized_by_principal_id,
        "finance.authorize_payment",
    )
    .await?;
    let row=sqlx::query("SELECT case_id,requested_by_principal_id,state,amount_minor,currency,direction FROM payment_intents WHERE id=$1")
        .bind(id).fetch_optional(&state.db).await.map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND,"payment intent not found".into()))?;
    if row.get::<String, _>("state") != "proposed" {
        return Err((
            StatusCode::CONFLICT,
            "payment intent is not proposed".into(),
        ));
    }
    if row.get::<Uuid, _>("requested_by_principal_id") == input.authorized_by_principal_id {
        return Err((
            StatusCode::FORBIDDEN,
            "requester cannot authorize the same payment intent".into(),
        ));
    }
    let updated=sqlx::query(
        r#"UPDATE payment_intents SET state='authorized',authorized_by_principal_id=$2,authorized_at=now(),updated_at=now()
           WHERE id=$1 AND state='proposed' AND executed_at IS NULL
           RETURNING case_id,amount_minor,currency,direction,authorized_at,executed_at"#,
    ).bind(id).bind(input.authorized_by_principal_id).fetch_optional(&state.db).await.map_err(internal)?
      .ok_or((StatusCode::CONFLICT,"payment intent could not be authorized".into()))?;
    let case_id = updated.get::<Uuid, _>("case_id");
    append_event(&state.db,case_id,Some(input.authorized_by_principal_id),"payment.authorized","payment_intent",Some(id),json!({"amount_minor":updated.get::<i64,_>("amount_minor"),"currency":updated.get::<String,_>("currency"),"executed":false})).await?;
    Ok(Json(json!({
        "id":id,"case_id":case_id,"state":"authorized","authorized_by_principal_id":input.authorized_by_principal_id,
        "authorized_at":updated.get::<DateTime<Utc>,_>("authorized_at"),"executed_at":updated.get::<Option<DateTime<Utc>>,_>("executed_at"),
        "execution_boundary":"external payment execution is not performed by the economic kernel"
    })))
}

async fn post_journal(
    State(state): State<AppState>,
    Json(input): Json<PostJournal>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    require_active_capability(&state.db, input.posted_by_principal_id, "finance.post").await?;
    ensure_case(&state.db, input.case_id).await?;
    if input.description.trim().is_empty()
        || input.idempotency_key.trim().is_empty()
        || input.lines.len() < 2
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "description, idempotency_key and at least two lines required".into(),
        ));
    }
    let currency = validate_currency(&input.currency)?;
    ensure_case_currency_compatible(&state.db, input.case_id, &currency).await?;
    let mut debit: i64 = 0;
    let mut credit: i64 = 0;
    for line in &input.lines {
        if line.debit_minor < 0
            || line.credit_minor < 0
            || !((line.debit_minor > 0 && line.credit_minor == 0)
                || (line.credit_minor > 0 && line.debit_minor == 0))
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "each journal line must contain exactly one positive debit or credit".into(),
            ));
        }
        debit = debit.checked_add(line.debit_minor).ok_or((
            StatusCode::BAD_REQUEST,
            "journal debit total overflow".into(),
        ))?;
        credit = credit.checked_add(line.credit_minor).ok_or((
            StatusCode::BAD_REQUEST,
            "journal credit total overflow".into(),
        ))?;
    }
    if debit != credit {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("journal is not balanced: debit={debit} credit={credit}"),
        ));
    }
    let mut tx = state.db.begin().await.map_err(internal)?;
    for line in &input.lines {
        let account = sqlx::query("SELECT currency,active FROM economic_accounts WHERE id=$1")
            .bind(line.account_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal)?
            .ok_or((
                StatusCode::BAD_REQUEST,
                "journal references unknown account".into(),
            ))?;
        if !account.get::<bool, _>("active") || account.get::<String, _>("currency") != currency {
            return Err((
                StatusCode::BAD_REQUEST,
                "journal account is inactive or currency-mismatched".into(),
            ));
        }
    }
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO journal_entries(id,case_id,description,currency,source_type,source_id,idempotency_key,posted_by_principal_id,posted_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,now())"#,
    ).bind(id).bind(input.case_id).bind(input.description.trim()).bind(&currency).bind(input.source_type.as_deref()).bind(input.source_id)
      .bind(input.idempotency_key.trim()).bind(input.posted_by_principal_id).execute(&mut *tx).await.map_err(conflict_or_internal)?;
    for (idx, line) in input.lines.iter().enumerate() {
        sqlx::query(
            "INSERT INTO journal_lines(entry_id,line_no,account_id,debit_minor,credit_minor,memo) VALUES($1,$2,$3,$4,$5,$6)",
        ).bind(id).bind((idx+1) as i32).bind(line.account_id).bind(line.debit_minor).bind(line.credit_minor).bind(&line.memo)
          .execute(&mut *tx).await.map_err(internal)?;
    }
    append_event_tx(
        &mut tx,
        input.case_id,
        Some(input.posted_by_principal_id),
        "journal.posted",
        "journal_entry",
        Some(id),
        json!({"currency":currency,"debit_minor":debit,"credit_minor":credit,"balanced":true}),
    )
    .await?;
    tx.commit().await.map_err(internal)?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"id":id,"case_id":input.case_id,"currency":currency,"debit_minor":debit,"credit_minor":credit,"balanced":true,"posted":true}),
        ),
    ))
}

async fn case_economic_summary(
    State(state): State<AppState>,
    Path(case_id): Path<Uuid>,
) -> ApiResult {
    ensure_case(&state.db, case_id).await?;
    let budget=sqlx::query("SELECT currency,amount_minor FROM economic_budgets WHERE case_id=$1 AND state='active' ORDER BY created_at DESC LIMIT 1")
        .bind(case_id).fetch_optional(&state.db).await.map_err(internal)?;
    let (currency, budget_minor) = match budget {
        Some(r) => (
            r.get::<String, _>("currency"),
            r.get::<i64, _>("amount_minor"),
        ),
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                "active economic budget not found for case".into(),
            ))
        }
    };
    let commitment_minor=scalar_i64(&state.db,
        "SELECT COALESCE(SUM(amount_minor),0)::BIGINT AS value FROM economic_commitments WHERE case_id=$1 AND currency=$2 AND state IN ('proposed','approved')",
        case_id,&currency).await?;
    let ar_open_minor=scalar_i64(&state.db,
        "SELECT COALESCE(SUM(amount_minor),0)::BIGINT AS value FROM economic_invoices WHERE case_id=$1 AND currency=$2 AND direction='receivable' AND state IN ('issued','approved')",
        case_id,&currency).await?;
    let ap_open_minor=scalar_i64(&state.db,
        "SELECT COALESCE(SUM(amount_minor),0)::BIGINT AS value FROM economic_invoices WHERE case_id=$1 AND currency=$2 AND direction='payable' AND state IN ('pending_approval','approved')",
        case_id,&currency).await?;
    let authorized_outbound_minor=scalar_i64(&state.db,
        "SELECT COALESCE(SUM(amount_minor),0)::BIGINT AS value FROM payment_intents WHERE case_id=$1 AND currency=$2 AND direction='outbound' AND state='authorized' AND executed_at IS NULL",
        case_id,&currency).await?;
    let econ=sqlx::query(
        r#"SELECT
             COALESCE(SUM(CASE WHEN a.account_type='expense' THEN (l.debit_minor-l.credit_minor) ELSE 0 END),0)::BIGINT AS expense_minor,
             COALESCE(SUM(CASE WHEN a.account_type='revenue' THEN (l.credit_minor-l.debit_minor) ELSE 0 END),0)::BIGINT AS revenue_minor
           FROM journal_entries e
           JOIN journal_lines l ON l.entry_id=e.id
           JOIN economic_accounts a ON a.id=l.account_id
           WHERE e.case_id=$1 AND e.currency=$2"#,
    ).bind(case_id).bind(&currency).fetch_one(&state.db).await.map_err(internal)?;
    let expense_minor = econ.get::<i64, _>("expense_minor");
    let revenue_minor = econ.get::<i64, _>("revenue_minor");
    let imbalance_count = sqlx::query(
        r#"SELECT COUNT(*)::BIGINT AS value FROM (
             SELECT e.id
             FROM journal_entries e JOIN journal_lines l ON l.entry_id=e.id
             WHERE e.case_id=$1 AND e.currency=$2
             GROUP BY e.id
             HAVING SUM(l.debit_minor) <> SUM(l.credit_minor)
           ) x"#,
    )
    .bind(case_id)
    .bind(&currency)
    .fetch_one(&state.db)
    .await
    .map_err(internal)?
    .get::<i64, _>("value");
    let budget_available_minor = budget_minor
        .saturating_sub(commitment_minor)
        .saturating_sub(expense_minor.max(0));
    Ok(Json(json!({
        "case_id":case_id,"currency":currency,"budget_minor":budget_minor,"commitments_minor":commitment_minor,
        "expense_minor":expense_minor,"revenue_minor":revenue_minor,"margin_minor":revenue_minor-expense_minor,
        "budget_available_minor":budget_available_minor,"ar_open_minor":ar_open_minor,"ap_open_minor":ap_open_minor,
        "authorized_outbound_minor":authorized_outbound_minor,"ledger_balanced":imbalance_count==0,
        "payment_execution":"external-only"
    })))
}

async fn scalar_i64(
    db: &PgPool,
    sql: &str,
    case_id: Uuid,
    currency: &str,
) -> Result<i64, (StatusCode, String)> {
    Ok(sqlx::query(sql)
        .bind(case_id)
        .bind(currency)
        .fetch_one(db)
        .await
        .map_err(internal)?
        .get::<i64, _>("value"))
}

async fn ensure_case(db: &PgPool, case_id: Uuid) -> Result<(), (StatusCode, String)> {
    if sqlx::query("SELECT 1 FROM company_cases WHERE id=$1")
        .bind(case_id)
        .fetch_optional(db)
        .await
        .map_err(internal)?
        .is_some()
    {
        Ok(())
    } else {
        Err((StatusCode::NOT_FOUND, "case not found".into()))
    }
}

async fn ensure_case_currency_compatible(
    db: &PgPool,
    case_id: Uuid,
    currency: &str,
) -> Result<(), (StatusCode, String)> {
    let row = sqlx::query("SELECT budget_currency FROM company_cases WHERE id=$1")
        .bind(case_id)
        .fetch_optional(db)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "case not found".into()))?;
    if let Some(case_currency) = row.get::<Option<String>, _>("budget_currency") {
        if case_currency.to_uppercase() != currency {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("currency {currency} does not match case budget currency {case_currency}"),
            ));
        }
    }
    Ok(())
}

async fn ensure_budget_headroom(
    db: &PgPool,
    case_id: Uuid,
    currency: &str,
    new_amount: i64,
) -> Result<(), (StatusCode, String)> {
    let budget=sqlx::query("SELECT amount_minor FROM economic_budgets WHERE case_id=$1 AND currency=$2 AND state='active' ORDER BY created_at DESC LIMIT 1")
        .bind(case_id).bind(currency).fetch_optional(db).await.map_err(internal)?
        .ok_or((StatusCode::CONFLICT,"active economic budget required before commitment".into()))?;
    let budget_minor = budget.get::<i64, _>("amount_minor");
    let commitments=scalar_i64(db,
        "SELECT COALESCE(SUM(amount_minor),0)::BIGINT AS value FROM economic_commitments WHERE case_id=$1 AND currency=$2 AND state IN ('proposed','approved')",
        case_id,currency).await?;
    let expense=sqlx::query(
        r#"SELECT COALESCE(SUM(CASE WHEN a.account_type='expense' THEN (l.debit_minor-l.credit_minor) ELSE 0 END),0)::BIGINT AS value
           FROM journal_entries e JOIN journal_lines l ON l.entry_id=e.id JOIN economic_accounts a ON a.id=l.account_id
           WHERE e.case_id=$1 AND e.currency=$2"#,
    ).bind(case_id).bind(currency).fetch_one(db).await.map_err(internal)?.get::<i64,_>("value");
    let available = budget_minor
        .saturating_sub(commitments)
        .saturating_sub(expense.max(0));
    if new_amount > available {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "commitment exceeds available budget: requested={new_amount} available={available}"
            ),
        ));
    }
    Ok(())
}

fn validate_currency(currency: &str) -> Result<String, (StatusCode, String)> {
    let c = currency.trim().to_uppercase();
    if c.len() != 3 || !c.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err((
            StatusCode::BAD_REQUEST,
            "currency must be a 3-letter uppercase ISO-style code".into(),
        ));
    }
    Ok(c)
}

async fn append_event(
    db: &PgPool,
    case_id: Uuid,
    principal_id: Option<Uuid>,
    kind: &str,
    object_type: &str,
    object_id: Option<Uuid>,
    payload: Value,
) -> Result<(), (StatusCode, String)> {
    sqlx::query("INSERT INTO economic_events(id,case_id,principal_id,kind,object_type,object_id,payload_json,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,now())")
        .bind(Uuid::new_v4()).bind(case_id).bind(principal_id).bind(kind).bind(object_type).bind(object_id).bind(payload.to_string())
        .execute(db).await.map_err(internal)?;
    Ok(())
}

async fn append_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    case_id: Uuid,
    principal_id: Option<Uuid>,
    kind: &str,
    object_type: &str,
    object_id: Option<Uuid>,
    payload: Value,
) -> Result<(), (StatusCode, String)> {
    sqlx::query("INSERT INTO economic_events(id,case_id,principal_id,kind,object_type,object_id,payload_json,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,now())")
        .bind(Uuid::new_v4()).bind(case_id).bind(principal_id).bind(kind).bind(object_type).bind(object_id).bind(payload.to_string())
        .execute(&mut **tx).await.map_err(internal)?;
    Ok(())
}

fn conflict_or_internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    let s = e.to_string();
    if s.contains("duplicate key value") || s.contains("unique constraint") {
        (StatusCode::CONFLICT, s)
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, s)
    }
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
