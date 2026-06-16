mod activity;
mod autologon;
mod bootstrap;
mod cli;
mod cmdlog;
mod config;
mod controlplane;
mod daemon;
mod env;
mod linker;
mod qr;
mod repair;
mod service;
mod session;
mod status;
mod store;
mod taskqueue;
mod tray;
mod ufo_config;
mod update;
mod util;
mod ws;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Cmd};
use config::Config;
use controlplane::ControlPlane;

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Short build id embedded at compile time (git sha / CI `$GITHUB_SHA` / "dev") — see build.rs. Lets
/// the dashboard and logs name the EXACT build, which `VERSION` alone can't (two builds, one version).
const GIT_SHA: &str = env!("UFOAGENT_GIT_SHA");

/// Human build string used in logs, `version`, `status`, and the reported `agent_version`.
pub fn build_string() -> String {
    format!("{VERSION} ({GIT_SHA})")
}

/// Short label for the running subcommand (for the startup build-stamp log line).
fn cmd_label(cmd: &Cmd) -> &'static str {
    match cmd {
        Cmd::Version => "version",
        Cmd::Status => "status",
        Cmd::Configure { .. } => "configure",
        Cmd::Link { .. } => "link",
        Cmd::Refresh => "refresh",
        Cmd::RunDaemon => "run-daemon",
        Cmd::Run { .. } => "run",
        Cmd::Bootstrap { .. } => "bootstrap",
        Cmd::Repair => "repair",
        Cmd::Activity { .. } => "activity",
        Cmd::Update { .. } => "update",
        Cmd::Service { .. } => "service",
        Cmd::Tray => "tray",
        Cmd::Autologon { .. } => "autologon",
    }
}

/// Set up logging. Always writes to the rotating log file; additionally mirrors to stderr
/// unless `quiet` (tray/service run without a console, so a stderr logger would either be lost
/// or spew into a parent shell — file-only is correct there).
fn init_logging(quiet: bool) {
    use simplelog::{
        ColorChoice, CombinedLogger, LevelFilter, SharedLogger, TermLogger, TerminalMode,
        WriteLogger,
    };
    let mut loggers: Vec<Box<dyn SharedLogger>> = Vec::new();
    if !quiet {
        loggers.push(TermLogger::new(
            LevelFilter::Info,
            simplelog::Config::default(),
            TerminalMode::Stderr,
            ColorChoice::Never,
        ));
    }
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

/// Make the console emit UTF-8 so the QR's block glyphs (and ✓) render and scan.
#[cfg(windows)]
fn set_utf8_console() {
    use windows::Win32::System::Console::SetConsoleOutputCP;
    // 65001 = CP_UTF8.
    unsafe {
        let _ = SetConsoleOutputCP(65001);
    }
}
#[cfg(not(windows))]
fn set_utf8_console() {}

fn main() -> Result<()> {
    set_utf8_console();
    // Bare `ufoagent.exe` (no subcommand) opens the tray manager.
    let cmd = Cli::parse().cmd.unwrap_or(Cmd::Tray);
    // The tray and service run without an attached console; keep their logging file-only so
    // they don't write to a parent shell (or a dead stderr).
    let quiet = matches!(cmd, Cmd::Tray | Cmd::Service { .. });
    init_logging(quiet);
    // Stamp the exact build at the top of every run — so `ufoagent.log` always answers "what's
    // running?" (especially for the long-lived service/tray) without needing the dashboard.
    log::info!(
        "ufoagent {} {} starting (cmd={})",
        build_string(),
        daemon::platform(),
        cmd_label(&cmd)
    );
    match cmd {
        Cmd::Version => println!("{}", build_string()),
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
            pause,
            force,
        } => cmd_link(control_plane, name, pause, force)?,
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
        Cmd::Bootstrap {
            ufo_home,
            git_ref,
            pause,
        } => {
            let result = bootstrap::bootstrap(ufo_home, &git_ref);
            match &result {
                Ok((home, _)) => println!("\n  UFO2 provisioned at {}\n", home.display()),
                Err(e) => {
                    // Into the log first — the console (--pause) vanishes when closed, but the
                    // verdict must survive in ufoagent.log (CI fails fast on this line).
                    log::error!("UFO2 setup failed: {e:#}");
                    // And a clear, human-readable failure on screen (the installer launches this
                    // in a visible console with --pause so the message stays up).
                    eprintln!("\n  UFO2 setup failed: {e:#}\n");
                    eprintln!("  You can retry later from the tray (Repair) or by running");
                    eprintln!("  `ufoagent bootstrap` from an elevated prompt.\n");
                }
            }
            if pause {
                pause_enter();
            }
            result?;
        }
        Cmd::Repair => {
            for line in repair::repair()? {
                println!("  {line}");
            }
        }
        Cmd::Activity { pause } => {
            println!("\n{}\n", activity::summarize());
            if pause {
                pause_enter();
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
            if apply {
                match update::maybe_self_update(&c, VERSION, Some(&s.min_version)) {
                    Ok(true) => {
                        println!("update downloaded + verified; installer launched (service will restart)")
                    }
                    Ok(false) => println!(
                        "no update applied (already current, disabled, or signature check refused — see log)"
                    ),
                    Err(e) => println!("update failed: {e}"),
                }
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

/// Hold a tray/installer-spawned console open until Enter (those windows close on process exit).
fn pause_enter() {
    println!("  Press Enter to close…");
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
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
    println!("version:       {}", build_string());
    println!("config_dir:    {}", config::config_dir().display());
}

fn cmd_link(
    control_plane: Option<String>,
    name: Option<String>,
    pause: bool,
    force: bool,
) -> Result<()> {
    let mut c = Config::load();
    if control_plane.is_some() {
        c.control_plane = control_plane;
    }
    c.save()?;

    // Avoid duplicate node records on reinstall: skip if already linked unless --force.
    if !force && store::get_token().is_some() {
        println!("\n  This machine is already linked. Re-run with --force to link again");
        println!("  (that registers a new, separate node).\n");
        if pause {
            pause_enter();
        }
        return Ok(());
    }

    let cp = ControlPlane::new(&c.control_plane_url(), None);
    let host = name.unwrap_or_else(|| hostname().unwrap_or_else(|| "windows-node".to_string()));
    let announce = |code: &str, uri: &str| {
        println!("\n  Link this machine — approve from your phone or another device.");
        println!(
            "  Scan the code below (or open the link), sign in, and confirm. Nothing to type.\n"
        );
        match qr::unicode(uri) {
            Ok(q) => println!("{q}"),
            Err(e) => log::warn!("could not render QR code: {e}"),
        }
        println!("  Pairing code:  {code}");
        println!("  Or open:       {uri}\n");
        println!("  Waiting for approval…");
    };
    let (agent_id, token) = linker::link(&cp, &host, &daemon::platform(), announce, 600)?;
    store::set_token(&token)?;
    println!("\n  Linked \u{2713}  as {host}  (agent {agent_id})\n");
    if pause {
        pause_enter();
    }
    Ok(())
}

fn cmd_run(task: String, request: Option<String>) -> Result<()> {
    // Gate on the target environment first. This is the shared local launcher — the tray "Run a
    // task…" menu, the tray's queue consumer, and the CLI all reach UFO2 through here — so without
    // this gate a run mid-install died with a raw "not provisioned". Now it refuses with the same
    // friendly message as the dashboard/remote path. ufo2 is the only env today.
    if let Some(reason) = env::gate(env::UFO2) {
        // The service already records queue/remote runs; don't double-log when invoked from the queue.
        if std::env::var("UFOAGENT_FROM_QUEUE").is_err() {
            cmdlog::record(
                "local",
                "run_task",
                request.as_deref(),
                "failed",
                Some(&reason),
            );
        }
        log::warn!("run gated: {reason}");
        eprintln!("{reason}");
        std::process::exit(1);
    }

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
    if let Some(r) = &request {
        cmd.arg("-r").arg(r);
    }
    // UTF-8 stdio for UFO2: with redirected output Python defaults to the ANSI codepage (cp1252),
    // and UFO2's post-task markdown log prints emoji (✅) — UnicodeEncodeError → exit 1 even after
    // the task succeeded (issue #8, second layer).
    cmd.env("PYTHONUTF8", "1");
    // When the tray drives this off the queue, run UFO2 headless too — no console flashing on the
    // desktop while it works (the tray already runs `ufoagent run` itself with no window).
    #[cfg(windows)]
    if std::env::var("UFOAGENT_FROM_QUEUE").is_ok() {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // Log (flushed per-record to ufoagent.log) as well as print: stdout block-buffers when
    // redirected to a file, so the log line is the reliable "UFO2 launched" signal.
    log::info!(
        "running UFO2: --task {task} request={request:?} (cwd={})",
        home.display()
    );
    println!(
        "running UFO2: -m ufo --task {task} (cwd={})",
        home.display()
    );
    let st = cmd.status()?;
    // Record in the on-node command history — unless the tray ran us off the remote queue, in which
    // case the service already logged it as a remote command (avoid double-counting).
    if std::env::var("UFOAGENT_FROM_QUEUE").is_err() {
        let status = if st.success() { "done" } else { "failed" };
        cmdlog::record("local", "run_task", request.as_deref(), status, None);
    }
    std::process::exit(st.code().unwrap_or(1));
}

fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
}
