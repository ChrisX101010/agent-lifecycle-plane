use crate::project::AgentRuntime;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    User,
    Agent,
    Client,
    Service,
    System,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalStatus {
    Active,
    Disabled,
    Quarantined,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    pub id: Uuid,
    pub name: String,
    pub kind: PrincipalKind,
    pub status: PrincipalStatus,
    pub created_at: DateTime<Utc>,
}

impl Principal {
    pub fn new(name: impl Into<String>, kind: PrincipalKind) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            kind,
            status: PrincipalStatus::Active,
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelRoutingStrategy {
    LowestCost,
    Balanced,
    HighestQuality,
    LowestLatency,
    LocalOnly,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub principal_id: Uuid,
    pub role: String,
    pub runtime: AgentRuntime,

    pub model_provider: Option<String>,
    pub model_name: Option<String>,

    pub routing_strategy: ModelRoutingStrategy,
    pub monthly_budget_eur: Option<f64>,
}

impl AgentDefinition {
    pub fn validate(&self) -> Result<(), String> {
        if self.role.trim().is_empty() {
            return Err("agent role cannot be empty".into());
        }

        if let Some(budget) = self.monthly_budget_eur {
            if budget <= 0.0 {
                return Err("agent monthly budget must be greater than zero".into());
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientDefinition {
    pub principal_id: Uuid,
    pub client_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    pub id: Uuid,
    pub name: String,
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn principal_is_active_by_default() {
        let p = Principal::new("research-01", PrincipalKind::Agent);

        assert_eq!(p.kind, PrincipalKind::Agent);
        assert_eq!(p.status, PrincipalStatus::Active);
    }

    #[test]
    fn invalid_agent_budget_is_rejected() {
        let p = Principal::new("research-01", PrincipalKind::Agent);

        let a = AgentDefinition {
            principal_id: p.id,
            role: "researcher".into(),
            runtime: AgentRuntime::Beam,
            model_provider: Some("local".into()),
            model_name: None,
            routing_strategy: ModelRoutingStrategy::LowestCost,
            monthly_budget_eur: Some(0.0),
        };

        assert!(a.validate().is_err());
    }
}
