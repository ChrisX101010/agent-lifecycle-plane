use alp_core::project::{ProjectManifest, ProjectProfile};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Parser)]
#[command(name = "alpctl")]
#[command(version)]
#[command(about = "Agent Lifecycle Plane — governed autonomy for real work")]
#[command(
    long_about = "Agent Lifecycle Plane keeps authority, evidence, workflow state, supervision, and economics outside the model. Use alpctl to inspect the machine, create and validate governed workspaces, launch Company Studio, run the reference demo, and execute release checks.",
    after_help = "FIRST RUN:
  alpctl doctor --system
  alpctl new my-company --template local
  cd my-company
  alpctl validate
  alpctl tree
  alpctl guide

Inside the Agent Lifecycle Plane repository:
  ./bootstrap.sh
  alpctl studio
  alpctl demo
  alpctl check"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a governed ALP workspace from a built-in profile.
    New {
        /// Project/workspace name.
        name: String,

        /// Destination directory. Defaults to ./<name>.
        #[arg(long)]
        path: Option<PathBuf>,

        /// Starting profile for runtime, services, security, and cost policy.
        #[arg(long, value_enum, default_value_t = Template::Local)]
        template: Template,
    },

    /// Validate an alp.yaml manifest and its policy invariants.
    Validate {
        #[arg(long, default_value = "alp.yaml")]
        file: PathBuf,
    },

    /// Explain the current manifest as a readable architecture tree.
    Tree {
        #[arg(long, default_value = "alp.yaml")]
        file: PathBuf,
    },

    /// Check project health, local tools, hardware, and model runtime availability.
    Doctor {
        #[arg(long, default_value = "alp.yaml")]
        file: PathBuf,
        /// Skip manifest checks and inspect only this machine/runtime.
        #[arg(long)]
        system: bool,
    },

    /// Print a concise first-run guide and safety model.
    Guide,

    /// Launch the terminal-native Company Studio.
    Studio {
        /// ALP control-plane base URL.
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        api: String,
        /// Skip the animated welcome screen.
        #[arg(long)]
        no_splash: bool,
    },

    /// Run the four end-to-end reference-company scenarios from the repository.
    Demo,

    /// Run the release-candidate gate; optionally include the exact staged public-tree gate.
    Check {
        #[arg(long)]
        public: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Template {
    Local,
    Budget,
    Secure,
    Kubernetes,
    Enterprise,
    MultiCloud,
}

impl Template {
    fn profile(self) -> ProjectProfile {
        match self {
            Self::Local => ProjectProfile::Local,
            Self::Budget => ProjectProfile::Budget,
            Self::Secure => ProjectProfile::Secure,
            Self::Kubernetes => ProjectProfile::KubernetesGitops,
            Self::Enterprise => ProjectProfile::Enterprise,
            Self::MultiCloud => ProjectProfile::MultiCloud,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::New {
            name,
            path,
            template,
        } => create_project(name, path, template),

        Commands::Validate { file } => validate(file),

        Commands::Tree { file } => tree(file),

        Commands::Doctor { file, system } => doctor(file, system),
        Commands::Guide => guide(),
        Commands::Studio { api, no_splash } => studio(api, no_splash),
        Commands::Demo => run_repo_script("scripts/alp14_reference_companies_gate.sh"),
        Commands::Check { public } => {
            run_repo_script("scripts/alp17_release_candidate_gate.sh")?;
            if public {
                run_repo_script("scripts/public_release_gate.sh")?;
            }
            Ok(())
        }
    }
}

fn create_project(name: String, path: Option<PathBuf>, template: Template) -> Result<()> {
    let root = path.unwrap_or_else(|| PathBuf::from(&name));

    if root.exists() {
        bail!("destination already exists: {}", root.display());
    }

    let manifest = ProjectManifest::template(name.clone(), template.profile());

    manifest
        .validate()
        .map_err(|e| anyhow::anyhow!(e.join("; ")))?;

    fs::create_dir_all(root.join("agents"))?;
    fs::create_dir_all(root.join("policies"))?;
    fs::create_dir_all(root.join("runtime"))?;
    fs::create_dir_all(root.join("infra"))?;
    fs::create_dir_all(root.join("security"))?;
    fs::create_dir_all(root.join("tests"))?;

    let yaml = serde_yaml::to_string(&manifest)?;

    fs::write(root.join("alp.yaml"), yaml)?;

    fs::write(
        root.join(".gitignore"),
        ".env\n.env.*\n!.env.example\ntarget/\nsecrets/\n*.pem\n*.key\n*.p12\n*.pfx\n*.jks\n*.log\n",
    )?;

    fs::write(root.join(".env.example"), "# ALP project environment\n")?;

    fs::write(
        root.join("README.md"),
        format!(
            "# {}\n\nGenerated by Agent Lifecycle Plane.\n\n\
             ```bash\nalpctl validate\nalpctl tree\nalpctl doctor\n```\n",
            name
        ),
    )?;

    fs::write(
        root.join("agents/README.md"),
        "# Agents\n\nAgent definitions belong here.\n",
    )?;

    fs::write(
        root.join("policies/README.md"),
        "# Policies\n\nProject policies belong here.\n",
    )?;

    println!("Created ALP project: {}", root.display());
    println!("Template: {}", enum_name(&manifest.profile));
    println!("Manifest: {}/alp.yaml", root.display());
    println!("ALP_DEFAULT_BLUEPRINT=PASS");

    Ok(())
}

fn read_manifest(path: &Path) -> Result<ProjectManifest> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;

    serde_yaml::from_str(&raw).with_context(|| format!("invalid ALP manifest {}", path.display()))
}

fn validate(path: PathBuf) -> Result<()> {
    let manifest = read_manifest(&path)?;

    match manifest.validate() {
        Ok(()) => {
            println!("Project: {}", manifest.project.name);
            println!("Manifest: {}", path.display());
            println!("ALP_MANIFEST_VALIDATION=PASS");
            Ok(())
        }

        Err(errors) => {
            for error in errors {
                eprintln!("ERROR: {error}");
            }

            bail!("manifest validation failed")
        }
    }
}

fn tree(path: PathBuf) -> Result<()> {
    let m = read_manifest(&path)?;

    m.validate().map_err(|e| anyhow::anyhow!(e.join("; ")))?;

    println!("{}", m.project.name);
    println!("├── profile        {}", enum_name(&m.profile));
    println!("├── environment    {}", m.project.environment);

    println!("├── runtime");
    println!("│   ├── primary    {}", enum_name(&m.runtime.primary));
    println!("│   └── isolation  {}", enum_name(&m.runtime.isolation));

    println!("├── provider");
    println!("│   └── primary    {}", enum_name(&m.provider.primary));

    println!("├── services");
    service_line("kubernetes", m.services.kubernetes);
    service_line("argocd", m.services.argocd);
    service_line("jenkins", m.services.jenkins);
    service_line("opentelemetry", m.services.opentelemetry);
    service_line("opencost", m.services.opencost);

    println!("├── security");
    println!(
        "│   ├── default-deny          {}",
        status(m.security.default_deny)
    );
    println!(
        "│   ├── dependency-admission  {}",
        status(m.security.dependency_admission)
    );
    println!(
        "│   └── critical-approval     {}",
        status(m.security.critical_risk_approval)
    );

    println!("└── cost");
    println!("    └── strategy   {}", enum_name(&m.cost.strategy));

    println!();
    println!("ALP_TREE=PASS");

    Ok(())
}

fn doctor(path: PathBuf, system_only: bool) -> Result<()> {
    let mut manifest_valid = true;
    if system_only {
        println!("Manifest             SKIPPED (--system)");
    } else if path.exists() {
        let m = read_manifest(&path)?;
        manifest_valid = m.validate().is_ok();
        println!(
            "Manifest             {}",
            if manifest_valid { "PASS" } else { "FAIL" }
        );
    } else {
        println!("Manifest             NOT_PRESENT");
    }
    let cargo = check_tool_status("cargo");
    let git = check_tool_status("git");
    let docker = check_tool_status("docker");
    let ollama = check_tool_status("ollama");
    let mem = system_memory_gib();
    println!("RAM                  {:.1} GiB", mem);
    let (gpu, vram) = gpu_info();
    if let Some(n) = gpu {
        println!("GPU                  {}", n);
        if let Some(v) = vram {
            println!("VRAM                 {:.1} GiB", v as f64 / 1024.0);
        }
    } else {
        println!("GPU                  not detected by nvidia-smi");
    }
    if ollama {
        if let Ok(o) = Command::new("ollama").arg("list").output() {
            let t = String::from_utf8_lossy(&o.stdout);
            println!(
                "Ollama models        {}",
                t.lines().skip(1).filter(|l| !l.trim().is_empty()).count()
            );
        }
    }
    let profile = if mem >= 64.0 || vram.unwrap_or(0) >= 24576 {
        "BUSINESS / TEAM local-first"
    } else if mem >= 32.0 || vram.unwrap_or(0) >= 12288 {
        "STARTUP / TEAM local-first"
    } else if mem >= 16.0 {
        "SOLO / STARTUP constrained local-first"
    } else {
        "SOLO constrained; prefer small quantized model or optional cloud burst"
    };
    println!("Recommended profile  {}", profile);
    if !manifest_valid || !cargo || !git || !docker {
        bail!("ALP project/system prerequisites are incomplete");
    }
    println!();
    println!("PROJECT_HEALTH=GREEN");
    println!("ALP_LOCAL_FIRST_READY=PASS");
    println!("ALP_DOCTOR=PASS");
    Ok(())
}
fn guide() -> Result<()> {
    println!("AGENT LIFECYCLE PLANE — FIRST RUN");
    println!();
    println!("1. Check this machine");
    println!("   alpctl doctor --system");
    println!();
    println!("2. Create a governed workspace");
    println!("   alpctl new my-company --template local");
    println!("   cd my-company");
    println!();
    println!("3. Validate and inspect the manifest");
    println!("   alpctl validate");
    println!("   alpctl tree");
    println!();
    println!("4. From the Agent Lifecycle Plane repository, start the local control plane");
    println!("   ./bootstrap.sh");
    println!();
    println!("5. Open Company Studio");
    println!("   alpctl studio");
    println!();
    println!("6. Prove the kernel end-to-end");
    println!("   alpctl demo");
    println!("   alpctl check");
    println!();
    println!("Safety model:");
    println!("  Models propose.");
    println!("  Deterministic policy authorizes.");
    println!("  Independent verification confirms outcomes.");
    println!("  v0.1 is local-first and single-tenant; read SECURITY.md before changing network boundaries.");
    println!();
    println!("ALP_GUIDE=PASS");
    Ok(())
}

fn studio(api: String, no_splash: bool) -> Result<()> {
    let mut direct = Command::new("alp-studio");
    direct.arg("--api").arg(&api);
    if no_splash {
        direct.arg("--no-splash");
    }

    match direct.status() {
        Ok(status) => {
            if status.success() {
                return Ok(());
            }
            bail!("alp-studio exited with status {status}");
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let mut cargo = Command::new("cargo");
    cargo
        .args(["run", "-p", "alp-studio", "--", "--api"])
        .arg(&api);
    if no_splash {
        cargo.arg("--no-splash");
    }
    let status = cargo
        .status()
        .context("alp-studio is not on PATH and the Cargo fallback could not be started")?;
    if !status.success() {
        bail!("Company Studio exited with status {status}");
    }
    Ok(())
}

fn run_repo_script(path: &str) -> Result<()> {
    let script = Path::new(path);
    if !script.exists() {
        bail!(
            "{path} was not found; run this command from the Agent Lifecycle Plane repository root"
        );
    }
    let status = Command::new("bash")
        .arg(script)
        .status()
        .with_context(|| format!("failed to start {path}"))?;
    if !status.success() {
        bail!("{path} failed with status {status}");
    }
    Ok(())
}

fn check_tool_status(name: &str) -> bool {
    let ok = Command::new(name).arg("--version").output().is_ok();
    println!("{:<20} {}", name, if ok { "PASS" } else { "NOT_INSTALLED" });
    ok
}
fn system_memory_gib() -> f64 {
    let raw = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kib = raw
        .lines()
        .find_map(|l| {
            l.strip_prefix("MemTotal:")?
                .split_whitespace()
                .next()?
                .parse::<f64>()
                .ok()
        })
        .unwrap_or(0.0);
    kib / 1024.0 / 1024.0
}
fn gpu_info() -> (Option<String>, Option<u64>) {
    let Ok(o) = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
    else {
        return (None, None);
    };
    if !o.status.success() {
        return (None, None);
    }
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    let mut p = line.split(',').map(str::trim);
    (
        p.next().filter(|s| !s.is_empty()).map(ToOwned::to_owned),
        p.next().and_then(|s| s.parse().ok()),
    )
}

fn service_line(name: &str, enabled: bool) {
    println!("│   ├── {:<15} {}", name, status(enabled));
}

fn status(enabled: bool) -> &'static str {
    if enabled {
        "ACTIVE"
    } else {
        "disabled"
    }
}

fn enum_name<T: Serialize>(value: &T) -> String {
    serde_yaml::to_string(value)
        .unwrap_or_else(|_| "unknown".into())
        .trim()
        .to_string()
}
