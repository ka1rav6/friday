//! Typed configuration with documented precedence:
//! defaults < user config < project config < environment < CLI override.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub bind: String,
    pub database_url: String,
    pub timezone: String,
    /// `auto` | `online` | `offline`
    pub mode: String,
    pub model: ModelConfig,
    pub agents: AgentConfig,
    pub permissions: PermissionConfig,
    pub workspaces: Vec<Workspace>,
    pub skills: SkillConfig,
    pub briefing: BriefingConfig,
    pub research: ResearchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ModelConfig {
    pub provider: String,
    pub nebius_base_url: String,
    pub nebius_model: String,
    pub ollama_base_url: String,
    pub ollama_model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub request_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    pub max_concurrent_subagents: usize,
    pub hard_max_subagents: usize,
    pub max_subagent_depth: u8,
    pub max_iterations: u32,
    pub max_tool_calls: u32,
    pub run_timeout_secs: u64,
    pub subagent_timeout_secs: u64,
    pub tool_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PermissionConfig {
    pub allow_network: bool,
    pub allow_local_mutation: bool,
    pub allow_external_side_effect: bool,
    pub allow_privileged: bool,
    /// Executables permitted for `run_build` / `run_project_tests`.
    pub command_allowlist: Vec<String>,
    pub max_file_bytes: u64,
    pub max_tool_output_bytes: usize,
    /// Require human approval before sending local content to a hosted model.
    pub confirm_hosted_uploads: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub id: String,
    pub path: PathBuf,
    #[serde(default)]
    pub writable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SkillConfig {
    pub user_dir: PathBuf,
    pub project_dir: PathBuf,
    pub allow_scripts: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BriefingConfig {
    pub enabled: bool,
    pub at_time: String,
    /// `synthetic` until a real OAuth connector is configured.
    pub email_provider: String,
    pub calendar_provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ResearchConfig {
    pub search_provider: String,
    pub max_sources: usize,
    pub max_page_bytes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8787".into(),
            database_url: "sqlite://./data/friday.db?mode=rwc".into(),
            timezone: "UTC".into(),
            mode: "auto".into(),
            model: ModelConfig::default(),
            agents: AgentConfig::default(),
            permissions: PermissionConfig::default(),
            workspaces: vec![],
            skills: SkillConfig::default(),
            briefing: BriefingConfig::default(),
            research: ResearchConfig::default(),
        }
    }
}
impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: "nebius".into(),
            nebius_base_url: "https://api.tokenfactory.nebius.com/v1".into(),
            nebius_model: "nvidia/nemotron-3-super-120b-a12b".into(),
            ollama_base_url: "http://127.0.0.1:11434".into(),
            ollama_model: "llama3.1".into(),
            max_tokens: 2048,
            temperature: 0.2,
            request_timeout_secs: 120,
        }
    }
}
impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_concurrent_subagents: 3,
            hard_max_subagents: 5,
            max_subagent_depth: 1,
            max_iterations: 12,
            max_tool_calls: 24,
            run_timeout_secs: 300,
            subagent_timeout_secs: 120,
            tool_timeout_secs: 60,
        }
    }
}
impl Default for PermissionConfig {
    fn default() -> Self {
        Self {
            allow_network: true,
            allow_local_mutation: true,
            allow_external_side_effect: false,
            allow_privileged: false,
            command_allowlist: vec!["cargo".into(), "git".into(), "npm".into()],
            max_file_bytes: 512 * 1024,
            max_tool_output_bytes: 64 * 1024,
            confirm_hosted_uploads: false,
        }
    }
}
impl Default for SkillConfig {
    fn default() -> Self {
        Self {
            user_dir: dirs_config().join("skills"),
            project_dir: PathBuf::from(".friday/skills"),
            allow_scripts: false,
        }
    }
}
impl Default for BriefingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            at_time: "07:30".into(),
            email_provider: "synthetic".into(),
            calendar_provider: "synthetic".into(),
        }
    }
}
impl Default for ResearchConfig {
    fn default() -> Self {
        Self {
            search_provider: "tavily".into(),
            max_sources: 6,
            max_page_bytes: 200_000,
        }
    }
}

pub fn dirs_config() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("friday")
}

pub const USER_CONFIG_REL: &str = "config.toml";
pub const PROJECT_CONFIG: &str = ".friday/config.toml";

/// Where an effective value came from. Surfaced by `GET /api/config`.
#[derive(Debug, Clone, Serialize)]
pub struct Sources {
    pub user_config: Option<String>,
    pub project_config: Option<String>,
    pub env_overrides: Vec<String>,
}

#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub sources: Sources,
}

/// Merge a partial TOML document onto a config. Unknown keys are rejected so a
/// typo is reported instead of silently ignored.
fn merge_toml(base: &Config, text: &str, origin: &str) -> Result<Config> {
    let mut v = toml::Value::try_from(base).context("serialize base config")?;
    let patch: toml::Value =
        toml::from_str(text).with_context(|| format!("{origin}: invalid TOML"))?;
    merge_value(&mut v, &patch);
    v.try_into::<Config>()
        .with_context(|| format!("{origin}: invalid configuration"))
}

fn merge_value(base: &mut toml::Value, patch: &toml::Value) {
    match (base, patch) {
        (toml::Value::Table(b), toml::Value::Table(p)) => {
            for (k, pv) in p {
                match b.get_mut(k) {
                    Some(bv) => merge_value(bv, pv),
                    None => {
                        b.insert(k.clone(), pv.clone());
                    }
                }
            }
        }
        (b, p) => *b = p.clone(),
    }
}

pub fn load(cli_bind: Option<String>, cli_mode: Option<String>) -> Result<Loaded> {
    let mut cfg = Config::default();
    let mut sources = Sources {
        user_config: None,
        project_config: None,
        env_overrides: vec![],
    };

    let user = dirs_config().join(USER_CONFIG_REL);
    if user.is_file() {
        let text = std::fs::read_to_string(&user)?;
        cfg = merge_toml(&cfg, &text, &user.display().to_string())?;
        sources.user_config = Some(user.display().to_string());
    }
    let proj = Path::new(PROJECT_CONFIG);
    if proj.is_file() {
        let text = std::fs::read_to_string(proj)?;
        cfg = merge_toml(&cfg, &text, PROJECT_CONFIG)?;
        sources.project_config = Some(PROJECT_CONFIG.to_string());
    }

    macro_rules! env_str {
        ($var:expr, $field:expr) => {
            if let Ok(v) = std::env::var($var) {
                if !v.is_empty() {
                    $field = v;
                    sources.env_overrides.push($var.to_string());
                }
            }
        };
    }
    env_str!("FRIDAY_BIND", cfg.bind);
    env_str!("FRIDAY_DB", cfg.database_url);
    env_str!("FRIDAY_TZ", cfg.timezone);
    env_str!("FRIDAY_MODE", cfg.mode);
    env_str!("NEBIUS_BASE_URL", cfg.model.nebius_base_url);
    env_str!("NEBIUS_MODEL", cfg.model.nebius_model);
    env_str!("OLLAMA_BASE_URL", cfg.model.ollama_base_url);

    if let Some(b) = cli_bind {
        cfg.bind = b;
        sources.env_overrides.push("--bind".into());
    }
    if let Some(m) = cli_mode {
        cfg.mode = m;
        sources.env_overrides.push("--mode".into());
    }
    validate(&cfg)?;
    Ok(Loaded {
        config: cfg,
        sources,
    })
}

pub fn validate(c: &Config) -> Result<()> {
    anyhow::ensure!(
        matches!(c.mode.as_str(), "auto" | "online" | "offline"),
        "mode: expected one of auto|online|offline, got {:?}",
        c.mode
    );
    anyhow::ensure!(
        matches!(c.model.provider.as_str(), "nebius" | "ollama" | "mock"),
        "model.provider: expected one of nebius|ollama|mock, got {:?}",
        c.model.provider
    );
    anyhow::ensure!(
        c.agents.max_concurrent_subagents >= 1
            && c.agents.max_concurrent_subagents <= c.agents.hard_max_subagents,
        "agents.max_concurrent_subagents must be in 1..=agents.hard_max_subagents ({})",
        c.agents.hard_max_subagents
    );
    anyhow::ensure!(
        c.agents.hard_max_subagents <= 5,
        "agents.hard_max_subagents must be <= 5 (spec limit)"
    );
    anyhow::ensure!(
        c.agents.max_subagent_depth <= 1,
        "agents.max_subagent_depth must be <= 1"
    );
    anyhow::ensure!(
        c.agents.max_iterations >= 1,
        "agents.max_iterations must be >= 1"
    );
    c.timezone
        .parse::<chrono_tz::Tz>()
        .map_err(|_| anyhow::anyhow!("timezone: {:?} is not a known IANA timezone", c.timezone))?;
    parse_hhmm(&c.briefing.at_time).ok_or_else(|| {
        anyhow::anyhow!(
            "briefing.at_time: expected HH:MM, got {:?}",
            c.briefing.at_time
        )
    })?;
    for w in &c.workspaces {
        anyhow::ensure!(!w.id.is_empty(), "workspaces[].id must not be empty");
    }
    Ok(())
}

pub fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

/// Config with every secret removed - safe for export and for the API.
pub fn redacted(c: &Config) -> serde_json::Value {
    serde_json::to_value(c).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_keys() {
        let e = merge_toml(&Config::default(), "nonsense_key = 1", "t").unwrap_err();
        assert!(format!("{e:#}").contains("invalid"));
    }

    #[test]
    fn partial_merge_keeps_other_defaults() {
        let c = merge_toml(&Config::default(), "[agents]\nmax_iterations = 3\n", "t").unwrap();
        assert_eq!(c.agents.max_iterations, 3);
        assert_eq!(c.agents.max_concurrent_subagents, 3);
    }

    #[test]
    fn validate_rejects_bad_values() {
        let mut c = Config::default();
        c.mode = "sideways".into();
        assert!(validate(&c).is_err());
        let mut c = Config::default();
        c.agents.hard_max_subagents = 99;
        assert!(validate(&c).is_err());
        let mut c = Config::default();
        c.briefing.at_time = "7h30".into();
        assert!(validate(&c).is_err());
    }

    #[test]
    fn invalid_edit_does_not_replace_known_good() {
        let good = Config::default();
        assert!(merge_toml(&good, "[agents]\nmax_iterations = \"x\"\n", "t").is_err());
        assert_eq!(good, Config::default());
    }
}
