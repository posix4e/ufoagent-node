use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "ufoagent",
    version,
    about = "UFOAgent node agent for Microsoft UFO2"
)]
pub struct Cli {
    /// Defaults to the system-tray manager when no subcommand is given (bare `ufoagent.exe`).
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Interactive device-code linking (shows a scannable QR to approve from any device).
    Link {
        #[arg(long)]
        control_plane: Option<String>,
        #[arg(long)]
        name: Option<String>,
        /// Wait for Enter before exiting, so a tray-spawned console stays readable.
        #[arg(long)]
        pause: bool,
        /// Link again even if already linked (creates a new, separate node record).
        #[arg(long)]
        force: bool,
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
        /// Wait for Enter before exiting (keeps an installer-spawned console readable,
        /// especially on failure).
        #[arg(long)]
        pause: bool,
    },
    /// Idempotently fix config / re-provision / refresh.
    Repair,
    /// Summarize what this node has been doing (LLM-powered, on-device).
    Activity {
        /// Keep the console open until Enter (for the tray-spawned window).
        #[arg(long)]
        pause: bool,
    },
    /// Check for updates.
    Update {
        #[arg(long)]
        apply: bool,
    },
    /// Show local status.
    Status,
    /// Print version.
    Version,
    /// Login-session node agent and tray UI (Windows).
    Tray,
    /// Configure unattended GUI mode by enabling Windows auto-logon (Windows).
    Autologon {
        /// Windows user to auto-logon. Omit to be prompted.
        #[arg(long)]
        user: Option<String>,
        /// Windows password for --user. Omit to be prompted without echo.
        #[arg(long)]
        password: Option<String>,
        /// Windows domain/computer name. Omit to use the current domain/computer.
        #[arg(long)]
        domain: Option<String>,
        /// Disable auto-logon.
        #[arg(long)]
        disable: bool,
        /// Wait for Enter before exiting, so an installer-spawned console stays readable.
        #[arg(long)]
        pause: bool,
    },
}
