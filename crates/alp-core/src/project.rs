use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub api_version: String,
    pub project: ProjectSpec,
    pub profile: ProjectProfile,
    pub runtime: RuntimeSpec,
    pub database: DatabaseSpec,
    pub provider: ProviderSpec,
    pub services: ServiceSpec,
    pub security: SecuritySpec,
    pub cost: CostSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSpec {
    pub name: String,
    pub environment: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectProfile {
    Local,
    Budget,
    Secure,
    KubernetesGitops,
    Enterprise,
    MultiCloud,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSpec {
    pub primary: AgentRuntime,
    pub fallback: Vec<AgentRuntime>,
    pub isolation: IsolationLevel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntime {
    Local,
    Beam,
    Docker,
    Kubernetes,
    Kata,
    Firecracker,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IsolationLevel {
    Process,
    Container,
    Microvm,
    Dedicated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSpec {
    pub provider: DatabaseProvider,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseProvider {
    LocalPostgres,
    Neon,
    ExistingPostgres,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSpec {
    pub primary: ProviderKind,
    pub fallback: Vec<ProviderKind>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Local,
    Auto,
    Hetzner,
    Aws,
    Azure,
    Gcp,
    Cloudflare,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSpec {
    pub kubernetes: bool,
    pub argocd: bool,
    pub jenkins: bool,
    pub opentelemetry: bool,
    pub opencost: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecuritySpec {
    pub default_deny: bool,
    pub dependency_admission: bool,
    pub high_risk_approval: bool,
    pub critical_risk_approval: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSpec {
    pub strategy: CostStrategy,
    pub monthly_budget_eur: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CostStrategy {
    LowestSuitable,
    Balanced,
    Performance,
    Manual,
}

impl ProjectManifest {
    pub fn template(name: impl Into<String>, profile: ProjectProfile) -> Self {
        let name = name.into();

        let mut manifest = Self {
            api_version: "alp.dev/v1alpha1".into(),
            project: ProjectSpec {
                name,
                environment: "development".into(),
            },
            profile,
            runtime: RuntimeSpec {
                primary: AgentRuntime::Beam,
                fallback: vec![AgentRuntime::Docker],
                isolation: IsolationLevel::Container,
            },
            database: DatabaseSpec {
                provider: DatabaseProvider::LocalPostgres,
            },
            provider: ProviderSpec {
                primary: ProviderKind::Local,
                fallback: vec![],
            },
            services: ServiceSpec {
                kubernetes: false,
                argocd: false,
                jenkins: false,
                opentelemetry: true,
                opencost: false,
            },
            security: SecuritySpec {
                default_deny: true,
                dependency_admission: true,
                high_risk_approval: true,
                critical_risk_approval: true,
            },
            cost: CostSpec {
                strategy: CostStrategy::LowestSuitable,
                monthly_budget_eur: None,
            },
        };

        match profile {
            ProjectProfile::Local => {}
            ProjectProfile::Budget => {
                manifest.provider.primary = ProviderKind::Auto;
                manifest.cost.monthly_budget_eur = Some(20.0);
            }
            ProjectProfile::Secure => {
                manifest.runtime.primary = AgentRuntime::Kata;
                manifest.runtime.isolation = IsolationLevel::Microvm;
            }
            ProjectProfile::KubernetesGitops => {
                manifest.runtime.primary = AgentRuntime::Kubernetes;
                manifest.services.kubernetes = true;
                manifest.services.argocd = true;
                manifest.services.opentelemetry = true;
                manifest.services.opencost = true;
            }
            ProjectProfile::Enterprise => {
                manifest.runtime.primary = AgentRuntime::Kubernetes;
                manifest.services.kubernetes = true;
                manifest.services.argocd = true;
                manifest.services.jenkins = true;
                manifest.services.opentelemetry = true;
                manifest.services.opencost = true;
            }
            ProjectProfile::MultiCloud => {
                manifest.runtime.primary = AgentRuntime::Kubernetes;
                manifest.provider.primary = ProviderKind::Auto;
                manifest.provider.fallback =
                    vec![ProviderKind::Hetzner, ProviderKind::Aws, ProviderKind::Gcp];
                manifest.services.kubernetes = true;
                manifest.services.argocd = true;
                manifest.services.opencost = true;
            }
            ProjectProfile::Custom => {}
        }

        manifest
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.project.name.trim().is_empty() {
            errors.push("project.name cannot be empty".into());
        }

        if self.project.environment.trim().is_empty() {
            errors.push("project.environment cannot be empty".into());
        }

        if self.services.argocd && !self.services.kubernetes {
            errors.push("Argo CD requires Kubernetes to be enabled".into());
        }

        if self.runtime.primary == AgentRuntime::Kubernetes && !self.services.kubernetes {
            errors.push("Kubernetes runtime requires services.kubernetes=true".into());
        }

        if !self.security.critical_risk_approval {
            errors.push("critical-risk human approval cannot be disabled".into());
        }

        if let Some(budget) = self.cost.monthly_budget_eur {
            if budget <= 0.0 {
                errors.push("monthly budget must be greater than zero".into());
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_blueprint_is_valid() {
        let manifest = ProjectManifest::template("demo", ProjectProfile::Local);

        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn argocd_without_kubernetes_is_rejected() {
        let mut manifest = ProjectManifest::template("demo", ProjectProfile::Local);

        manifest.services.argocd = true;

        assert!(manifest.validate().is_err());
    }

    #[test]
    fn critical_approval_cannot_be_disabled() {
        let mut manifest = ProjectManifest::template("demo", ProjectProfile::Local);

        manifest.security.critical_risk_approval = false;

        assert!(manifest.validate().is_err());
    }
}
