use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "ufoagent",
    version,
    about = "UFOAgent node agent for Microsoft UFO2"
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Interactive device-code linking.
    Link {
        #[arg(long)]
        control_plane: Option<String>,
        #[arg(long)]
        name: Option<String>,
    },
    /// Non-interactive configure (control plane / token / ufo_home).
    Configure {
        #[arg(long)]
        control_plane: Option<String>,
        #[arg(long)]
        agent_token: Option<String>,
        #[arg(long)]
        ufo_home: Option<String>,
    },
    /// Fetch a credential and write agents.yaml.
    Refresh,
    /// Foreground refresh + heartbeat loop (the service body).
    RunDaemon,
    /// Refresh credential then run a UFO2 task.
    Run {
        #[arg(long)]
        task: String,
        #[arg(short = 'r', long)]
        request: Option<String>,
    },
    /// Install UFO2 + dependencies into a managed home (one-time, large).
    Bootstrap {
        #[arg(long)]
        ufo_home: Option<String>,
        #[arg(long = "ref", default_value = "main")]
        git_ref: String,
    },
    /// Idempotently fix config / re-provision / refresh.
    Repair,
    /// Check for updates.
    Update {
        #[arg(long)]
        apply: bool,
    },
    /// Show local status.
    Status,
    /// Print version.
    Version,
    /// Windows service control (install/uninstall/run).
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// System-tray manager UI (Windows).
    Tray,
}

#[derive(Subcommand)]
pub enum ServiceAction {
    Install,
    Uninstall,
    Run,
}
