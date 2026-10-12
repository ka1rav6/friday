//! FRIDAY - Personal AI Operating System.
//!
//! One binary serves the HTTP/WebSocket API consumed by the React frontend and
//! by the CLI subcommands, so every surface reads the same configuration and
//! the same tool registry.
use anyhow::Result;
use clap::{Parser, Subcommand};
use friday::{agents, api, config, db, scheduler, skills, state, tools, util};
use state::AppState;

#[derive(Parser)]
#[command(
    name = "friday",
    version,
    about = "FRIDAY - Personal AI Operating System"
)]
struct Cli {
    /// Override the listen address.
    #[arg(long, global = true)]
    bind: Option<String>,
    /// Override connectivity mode: auto | online | offline.
    #[arg(long, global = true)]
    mode: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (default).
    Serve,
    /// Print the effective configuration and where each source came from.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage skills without editing application source.
    Skills {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Print health, model routing and counts.
    Status,
    /// List the tool catalogue. `--markdown` generates docs/tools.md.
    Tools {
        #[arg(long)]
        markdown: bool,
    },
    /// Generate today's briefing now.
    Brief {
        #[arg(long)]
        force: bool,
    },
    /// Ask FRIDAY one question and print the answer.
    Ask { prompt: Vec<String> },
    /// Connect or disconnect an account.
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Report which voice, screen and OCR backends this machine has.
    Media,
}

#[derive(Subcommand)]
enum ConfigAction {
    Show,
    /// Validate the current files and report the first actionable problem.
    Validate,
    /// Print a commented default config you can save and edit.
    Init,
}

#[derive(Subcommand)]
enum AuthAction {
    /// Grant FRIDAY read-only access to Gmail.
    Gmail {
        /// Delete the stored token instead of connecting.
        #[arg(long)]
        forget: bool,
        /// Print the consent URL instead of opening a browser.
        #[arg(long)]
        no_open: bool,
    },
}

#[derive(Subcommand)]
enum SkillAction {
    List,
    /// Inspect a package directory without installing it.
    Inspect {
        path: String,
    },
    /// Scaffold a new skill from the built-in template.
    New {
        name: String,
        #[arg(long, default_value = "Describe this skill.")]
        description: String,
        #[arg(long, default_value = "project")]
        scope: String,
    },
    /// Import an existing package directory.
    Add {
        path: String,
        #[arg(long, default_value = "project")]
        scope: String,
        #[arg(long)]
        enable: bool,
    },
    /// Discover packages already present in the skill directories.
    Rescan,
    Enable {
        name: String,
    },
    Disable {
        name: String,
    },
    /// Detach a skill. Files are kept unless --delete-files is given.
    Remove {
        name: String,
        #[arg(long)]
        delete_files: bool,
    },
    Run {
        name: String,
        #[arg(long)]
        input: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "friday=info,tower_http=warn".into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let loaded = config::load(cli.bind.clone(), cli.mode.clone())?;
    let bind = loaded.config.bind.clone();
    let db = db::connect(&loaded.config.database_url).await?;
    let state = AppState::new(db, loaded);

    scheduler::register_builtins(&state).await?;
    skills::rescan(&state).await.ok();
    let recovered = agents::recover_interrupted(&state).await?;
    if recovered > 0 {
        tracing::warn!("marked {recovered} interrupted task(s)/run(s) as failed after restart");
    }

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(state, &bind).await,
        Command::Status => {
            let ctx = tools::ToolCtx::root(state.clone(), None);
            let v = tools::execute(&ctx, "get_system_status", serde_json::json!({})).await;
            println!("{}", serde_json::to_string_pretty(&v.value)?);
            Ok(())
        }
        Command::Tools { markdown } => {
            let defs = state.registry.defs();
            if markdown {
                print!("{}", tools_markdown(&defs));
            } else {
                println!("{}", serde_json::to_string_pretty(&defs)?);
            }
            Ok(())
        }
        Command::Auth { action } => auth_cmd(&state, action).await,
        Command::Media => {
            let ctx = tools::ToolCtx::root(state.clone(), None);
            let v = tools::execute(&ctx, "get_media_capabilities", serde_json::json!({})).await;
            println!("{}", serde_json::to_string_pretty(&v.value)?);
            Ok(())
        }
        Command::Config { action } => config_cmd(&state, action).await,
        Command::Skills { action } => skills_cmd(&state, action).await,
        Command::Brief { force } => {
            scheduler::ensure_default_job(&state).await?;
            let v = scheduler::run_job(&state, "morning-briefing", force).await?;
            println!("{}", serde_json::to_string_pretty(&v)?);
            Ok(())
        }
        Command::Ask { prompt } => {
            let text = prompt.join(" ");
            anyhow::ensure!(!text.trim().is_empty(), "nothing to ask");
            let out = agents::handle_message(&state, &util::uid(), &text).await?;
            println!("{}", out.content);
            if out.status != "completed" {
                eprintln!("\n[status: {}]", out.status);
            }
            Ok(())
        }
    }
}

async fn serve(state: AppState, bind: &str) -> Result<()> {
    scheduler::ensure_default_job(&state).await?;
    scheduler::spawn_loop(state.clone()).await;

    let cfg = state.cfg().await;
    let router = state.router.read().await.clone();
    match router.select(&cfg.mode) {
        Ok((_, r)) => {
            tracing::info!(provider = %r.provider, model = %r.model, "model routing: {}", r.reason)
        }
        Err(e) => tracing::warn!("no model provider available: {e}"),
    }
    if router.hosted.is_none() {
        tracing::warn!(
            "NEBIUS_API_KEY is not set: the hosted NVIDIA Nemotron model is unavailable. \
             Copy .env.example to .env and add your Token Factory key."
        );
    }

    let app = api::router(state.clone());
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("FRIDAY listening on http://{bind}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}

async fn config_cmd(state: &AppState, action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Show => {
            let cfg = state.cfg().await;
            let sources = state.config_sources.read().await;
            println!("# effective configuration (secrets are never shown)");
            println!("{}", toml::to_string_pretty(&cfg)?);
            println!("# precedence: defaults < user config < project config < environment < cli");
            println!("# user config:    {:?}", sources.user_config);
            println!("# project config: {:?}", sources.project_config);
            println!("# env overrides:  {:?}", sources.env_overrides);
        }
        ConfigAction::Validate => {
            config::validate(&state.cfg().await)?;
            println!("configuration is valid");
        }
        ConfigAction::Init => {
            println!("# Save as {} and edit.", config::PROJECT_CONFIG);
            println!("{}", toml::to_string_pretty(&config::Config::default())?);
        }
    }
    Ok(())
}

async fn skills_cmd(state: &AppState, action: SkillAction) -> Result<()> {
    let out = match action {
        SkillAction::List => serde_json::json!({"skills": skills::list(state).await?}),
        SkillAction::Inspect { path } => {
            skills::inspect(state, std::path::Path::new(&path)).await?
        }
        SkillAction::New {
            name,
            description,
            scope,
        } => skills::create_from_template(state, &name, &description, &scope).await?,
        SkillAction::Add {
            path,
            scope,
            enable,
        } => skills::install(state, std::path::Path::new(&path), &scope, enable).await?,
        SkillAction::Rescan => skills::rescan(state).await?,
        SkillAction::Enable { name } => skills::set_enabled(state, &name, true).await?,
        SkillAction::Disable { name } => skills::set_enabled(state, &name, false).await?,
        SkillAction::Remove { name, delete_files } => {
            skills::remove(state, &name, delete_files).await?
        }
        SkillAction::Run { name, input } => {
            skills::run(state, &name, input.as_deref(), None).await?
        }
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// Renders the live registry as the table in `docs/tools.md`, so the document
/// cannot drift from the code.
fn tools_markdown(defs: &[tools::ToolDefinition]) -> String {
    let mut out = String::from(
        "<!-- Generated by `cargo run -- tools --markdown`. Do not edit by hand. -->\n\
         # Tool catalogue\n\n\
         Every tool below is implemented and reachable by the agent. The model may *request* a\n\
         tool; the runtime decides whether it runs, after checking the subagent grant, the\n\
         permission policy, offline mode and the declared argument schema. Each call is timed,\n\
         size-bounded and written to `tool_calls` and `audit_log`.\n\n\
         | Tool | Permission | Network | Timeout | Description |\n\
         | --- | --- | --- | --- | --- |\n",
    );
    for d in defs {
        out.push_str(&format!(
            "| `{}` | {} | {} | {}s | {} |\n",
            d.name,
            d.permission.as_str(),
            if d.network { "yes" } else { "no" },
            d.timeout_ms / 1000,
            d.description.replace('\n', " ").replace('|', "\\|"),
        ));
    }
    out.push_str(&format!("\n**{} tools implemented.**\n", defs.len()));
    out.push_str(
        "\n## Permission categories\n\n\
         | Category | Meaning | Policy flag | Default |\n| --- | --- | --- | --- |\n\
         | `read_only` | Reads local state only | always allowed | on |\n\
         | `local_mutation` | Writes local state (memories, tasks, files, builds) | `permissions.allow_local_mutation` | on |\n\
         | `network` | Leaves the machine | `permissions.allow_network` | on |\n\
         | `external_side_effect` | Visible to others (sending mail, posting) | `permissions.allow_external_side_effect` | off |\n\
         | `privileged` | Privileged system access | `permissions.allow_privileged` | off |\n\
         \nA permission category is not an authorization. Each invocation is additionally checked\n\
         against the caller's grant, the workspace scope and the current connectivity mode.\n",
    );
    out
}

async fn auth_cmd(state: &AppState, action: AuthAction) -> Result<()> {
    match action {
        AuthAction::Gmail { forget: true, .. } => {
            let removed = friday::gmail::forget_token()?;
            println!(
                "{}",
                if removed {
                    "Stored Gmail token deleted. Also revoke FRIDAY at \
                     https://myaccount.google.com/permissions to invalidate it on Google's side."
                } else {
                    "No stored Gmail token to delete."
                }
            );
            state.audit("user", "gmail", "forget_token", "token deleted by user", None).await;
            Ok(())
        }
        AuthAction::Gmail { no_open, .. } => {
            let cfg = state.cfg().await;
            let auth = friday::gmail::begin(cfg.briefing.oauth_port).await?;

            println!("FRIDAY is requesting READ-ONLY Gmail access ({}).", friday::gmail::SCOPE);
            println!("It cannot send, delete or modify mail with this scope.\n");
            println!("Open this URL and grant access:\n\n  {}\n", auth.url);
            if !no_open {
                // Best effort: if no opener exists, the printed URL is the fallback.
                for opener in ["xdg-open", "open", "start"] {
                    if friday::media::which(opener).is_some() {
                        let _ = friday::media::run(opener, &[auth.url.as_str()], 10, None).await;
                        break;
                    }
                }
            }
            println!("Waiting for the redirect on 127.0.0.1:{}…", cfg.briefing.oauth_port);

            let token = friday::gmail::complete(
                auth,
                &state.http,
                std::time::Duration::from_secs(300),
            )
            .await?;
            state.audit("user", "gmail", "connected", &format!("scope={}", token.scope), None).await;

            println!("\nConnected. Token stored in {}.", friday::gmail::token_path().display());
            println!("Set briefing.email_provider = \"gmail\" to use it, then run `friday brief`.");
            Ok(())
        }
    }
}
