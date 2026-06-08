mod autologon;
mod bootstrap;
mod cli;
mod config;
mod controlplane;
mod daemon;
mod linker;
mod repair;
mod service;
mod status;
mod store;
mod tray;
mod ufo_config;
mod update;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Cmd};
use config::Config;
use controlplane::ControlPlane;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn init_logging() {
    use simplelog::{
        ColorChoice, CombinedLogger, LevelFilter, SharedLogger, TermLogger, TerminalMode,
        WriteLogger,
    };
    let mut loggers: Vec<Box<dyn SharedLogger>> = vec![TermLogger::new(
        LevelFilter::Info,
        simplelog::Config::default(),
        TerminalMode::Stderr,
        ColorChoice::Never,
    )];
    let log_dir = config::config_dir().join("logs");
    if std::fs::create_dir_all(&log_dir).is_ok() {
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("ufoagent.log"))
        {
            loggers.push(WriteLogger::new(
                LevelFilter::Info,
                simplelog::Config::default(),
                file,
            ));
        }
    }
    let _ = CombinedLogger::init(loggers);
}

fn main() -> Result<()> {
    init_logging();
    match Cli::parse().cmd {
        Cmd::Version => println!("{VERSION}"),
        Cmd::Status => cmd_status(),
        Cmd::Configure {
            control_plane,
            agent_token,
            ufo_home,
        } => {
            let mut c = Config::load();
            if control_plane.is_some() {
                c.control_plane = control_plane;
            }
            if ufo_home.is_some() {
                c.ufo_home = ufo_home;
            }
            c.save()?;
            if let Some(t) = agent_token {
                store::set_token(&t)?;
            }
            println!("configured");
        }
        Cmd::Link {
            control_plane,
            name,
        } => cmd_link(control_plane, name)?,
        Cmd::Refresh => {
            let c = Config::load();
            let cp = ControlPlane::new(&c.control_plane_url(), store::get_token());
            let cred = daemon::refresh_once(&cp, &c.ufo_home_path())?;
            println!(
                "credential written: lease={} model={} expires_at={}",
                cred.lease_id, cred.model, cred.expires_at
            );
        }
        Cmd::RunDaemon => daemon::run_daemon(VERSION, || false)?,
        Cmd::Run { task, request } => cmd_run(task, request)?,
        Cmd::Bootstrap { ufo_home, git_ref } => {
            let (home, _) = bootstrap::bootstrap(ufo_home, &git_ref)?;
            println!("UFO2 provisioned at {}", home.display());
        }
        Cmd::Repair => {
            for line in repair::repair()? {
                println!("  {line}");
            }
        }
        Cmd::Update { apply } => {
            let c = Config::load();
            let cp = ControlPlane::new(&c.control_plane_url(), store::get_token());
            let s = update::check(&cp, VERSION)?;
            println!(
                "current={} min_version={} update_required={}",
                s.current, s.min_version, s.update_required
            );
            if apply && s.update_required {
                println!("auto-apply not implemented yet — re-run the installer");
            }
        }
        Cmd::Service { action } => service::run_action(&action)?,
        Cmd::Tray => tray::run(VERSION)?,
        Cmd::Autologon {
            user,
            password,
            domain,
            disable,
        } => autologon::run(&user, password.as_deref(), domain.as_deref(), disable)?,
    }
    Ok(())
}

fn cmd_status() {
    let c = Config::load();
    let unset = || "(unset)".to_string();
    println!(
        "control_plane: {}",
        c.control_plane.clone().unwrap_or_else(unset)
    );
    println!(
        "ufo_home:      {}",
        c.ufo_home.clone().unwrap_or_else(unset)
    );
    println!("python:        {}", c.python.clone().unwrap_or_else(unset));
    println!(
        "linked:        {}",
        if store::get_token().is_some() {
            "yes"
        } else {
            "no"
        }
    );
    println!("version:       {VERSION}");
    println!("config_dir:    {}", config::config_dir().display());
}

fn cmd_link(control_plane: Option<String>, name: Option<String>) -> Result<()> {
    let mut c = Config::load();
    if control_plane.is_some() {
        c.control_plane = control_plane;
    }
    c.save()?;
    let cp = ControlPlane::new(&c.control_plane_url(), None);
    let host = name.unwrap_or_else(|| hostname().unwrap_or_else(|| "windows-node".to_string()));
    let announce = |code: &str, uri: &str| {
        println!("\n  Link this machine: open  {uri}\n  and confirm the code:  {code}\n");
    };
    let (agent_id, token) = linker::link(&cp, &host, &daemon::platform(), announce, 600)?;
    store::set_token(&token)?;
    println!("linked: agent={agent_id} name={host}");
    Ok(())
}

fn cmd_run(task: String, request: Option<String>) -> Result<()> {
    let c = Config::load();
    let cp = ControlPlane::new(&c.control_plane_url(), store::get_token());
    let home = c.ufo_home_path();
    daemon::refresh_once(&cp, &home)?;
    let python = c
        .python
        .clone()
        .ok_or_else(|| anyhow::anyhow!("UFO2 not provisioned; run `ufoagent bootstrap` first"))?;
    let mut cmd = std::process::Command::new(python);
    cmd.args(["-m", "ufo", "--task", &task]).current_dir(&home);
    if let Some(r) = request {
        cmd.arg("-r").arg(r);
    }
    println!(
        "running UFO2: -m ufo --task {task} (cwd={})",
        home.display()
    );
    let st = cmd.status()?;
    std::process::exit(st.code().unwrap_or(1));
}

fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
}
