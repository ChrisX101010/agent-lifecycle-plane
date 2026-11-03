pub mod identity;
pub mod project;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    pub fn requires_human_approval(self) -> bool {
        matches!(self, Self::High | Self::Critical)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    AwaitingApproval,
    Approved,
    Running,
    SubmittedUnverified,
    Verified,
    VerificationRejected,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    Local,
    Docker,
    Kubernetes,
    Serverless,
    Edge,
    FlociAws,
    FlociAzure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub risk: RiskLevel,
    pub runtime: RuntimeKind,
    pub state: TaskState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Task {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        risk: RiskLevel,
        runtime: RuntimeKind,
    ) -> Self {
        let now = Utc::now();
        let state = if risk.requires_human_approval() {
            TaskState::AwaitingApproval
        } else {
            TaskState::Queued
        };

        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            description: description.into(),
            risk,
            runtime,
            state,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub id: Uuid,
    pub task_id: Uuid,
    pub approved_by: String,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: Uuid,
    pub task_id: Option<Uuid>,
    pub kind: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Incident {
    pub id: Uuid,
    pub task_id: Option<Uuid>,
    pub severity: RiskLevel,
    pub category: String,
    pub summary: String,
    pub auto_remediation_allowed: bool,
    pub resolved: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionReceipt {
    pub runtime: RuntimeKind,
    pub external_id: String,
    pub accepted_at: DateTime<Utc>,
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime rejected task: {0}")]
    Rejected(String),

    #[error("runtime unavailable: {0}")]
    Unavailable(String),
}

#[async_trait]
pub trait RuntimeAdapter: Send + Sync {
    fn kind(&self) -> RuntimeKind;
    async fn submit(&self, task: &Task) -> Result<ExecutionReceipt, RuntimeError>;
}

#[derive(Default)]
pub struct LocalRuntime;

#[async_trait]
impl RuntimeAdapter for LocalRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Local
    }

    async fn submit(&self, task: &Task) -> Result<ExecutionReceipt, RuntimeError> {
        if task.risk.requires_human_approval() && task.state != TaskState::Approved {
            return Err(RuntimeError::Rejected(
                "high-risk task requires explicit approval".into(),
            ));
        }

        Ok(ExecutionReceipt {
            runtime: RuntimeKind::Local,
            external_id: format!("local-{}", task.id),
            accepted_at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_risk_task_queues_immediately() {
        let task = Task::new("safe", "demo", RiskLevel::Low, RuntimeKind::Local);
        assert_eq!(task.state, TaskState::Queued);
    }

    #[test]
    fn high_risk_task_waits_for_approval() {
        let task = Task::new("dangerous", "demo", RiskLevel::High, RuntimeKind::Local);
        assert_eq!(task.state, TaskState::AwaitingApproval);
    }
}
