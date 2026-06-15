//! Provision Microsoft UFO2 into a managed home: download source zip + venv + pip install.
//! No external tools required — fetches the GitHub source archive over HTTP and unpacks it with
//! Windows' built-in tar, and auto-installs Python if none is present. UFO2 stays Python; this
//! just sets it up and records ufo_home + the venv python.

use anyhow::{anyhow, bail, Context, Result};
use log::info;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;
use crate::controlplane::USER_AGENT;

/// UFO2 pins some packages with no Python 3.11 wheel (build-from-source fails on modern
/// setuptools). Rewrite those to wheel-backed versions.
const PIN_OVERRIDES: &[(&str, &str)] = &[("pandas==1.4.3", "pandas==1.5.3")];

/// GitHub source-archive endpoint. No `git` needed — we download the branch zip and unpack it
/// with the `tar.exe` shipped in Windows (libarchive, reads zips).
const UFO_ARCHIVE_BASE: &str = "https://github.com/microsoft/UFO/archive/refs/heads";

/// Pinned CPython used when no system interpreter is found (UFO2 is Python; can't drop it).
#[cfg(windows)]
const PYTHON_VERSION: &str = "3.11.9";

fn venv_python(home: &Path) -> PathBuf {
    if cfg!(windows) {
        home.join(".venv").join("Scripts").join("python.exe")
    } else {
        home.join(".venv").join("bin").join("python")
    }
}

/// Stream a URL to `dest`. Uses the custom User-Agent (Cloudflare/GitHub block the default).
fn download(url: &str, dest: &Path) -> Result<()> {
    let resp = ureq::get(url)
        .set("user-agent", USER_AGENT)
        .call()
        .map_err(|e| anyhow!("download failed for {url}: {e}"))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut reader = resp.into_reader();
    let mut file =
        std::fs::File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    std::io::copy(&mut reader, &mut file).with_context(|| format!("writing {url}"))?;
    Ok(())
}

/// Download the UFO2 source archive for `git_ref` and unpack it into `home`, stripping the
/// `UFO-<ref>/` directory the archive nests everything under.
fn fetch_ufo(home: &Path, git_ref: &str) -> Result<()> {
    std::fs::create_dir_all(home)?;
    let url = format!("{UFO_ARCHIVE_BASE}/{git_ref}.zip");
    let safe: String = git_ref
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let zip = home
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("ufo-{safe}.zip"));
    info!("downloading microsoft/UFO@{git_ref} -> {}", home.display());
    download(&url, &zip)?;
    // Windows' built-in tar (bsdtar) reads zips; --strip-components=1 drops the top folder.
    let mut c = Command::new("tar");
    c.arg("-xf").arg(&zip).arg("-C").arg(home);
    c.arg("--strip-components=1");
    let r = run(c, "extract UFO archive");
    let _ = std::fs::remove_file(&zip);
    r?;
    if !home.join("requirements.txt").exists() {
        bail!(
            "UFO archive extracted but requirements.txt is missing under {} — extraction may have failed",
            home.display()
        );
    }
    Ok(())
}

/// Probe for a usable system interpreter via the launcher / PATH, then known install dirs.
fn find_python() -> Option<PathBuf> {
    for cand in [
        vec!["py", "-3.11"],
        vec!["py", "-3.10"],
        vec!["py", "-3"],
        vec!["python"],
        vec!["python3"],
    ] {
        let mut c = Command::new(cand[0]);
        for a in &cand[1..] {
            c.arg(a);
        }
        c.arg("-c").arg("import sys;print(sys.executable)");
        if let Ok(o) = c.output() {
            if o.status.success() {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
        }
    }
    // A just-installed interpreter won't be on *this* process's PATH yet — check the standard
    // install locations directly.
    #[cfg(windows)]
    for dir in python_install_dirs() {
        let p = dir.join("python.exe");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg(windows)]
fn python_install_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(pf) = std::env::var("ProgramFiles") {
        v.push(PathBuf::from(pf).join("Python311"));
    }
    if let Ok(la) = std::env::var("LocalAppData") {
        v.push(
            PathBuf::from(la)
                .join("Programs")
                .join("Python")
                .join("Python311"),
        );
    }
    v
}

/// Download and silently install the official python.org build. Windows-only (UFO2 only runs
/// on Windows; elsewhere we just ask the user to install Python).
#[cfg(windows)]
fn install_python(scratch_dir: &Path) -> Result<()> {
    let url = format!(
        "https://www.python.org/ftp/python/{PYTHON_VERSION}/python-{PYTHON_VERSION}-amd64.exe"
    );
    let installer = scratch_dir.join(format!("python-{PYTHON_VERSION}-amd64.exe"));
    info!("downloading Python {PYTHON_VERSION} installer…");
    download(&url, &installer)?;
    info!("installing Python {PYTHON_VERSION} (silent, all users)…");
    let mut c = Command::new(&installer);
    c.args([
        "/quiet",
        "InstallAllUsers=1",
        "PrependPath=1",
        "Include_launcher=1",
    ]);
    let r = run(c, "Python installer");
    let _ = std::fs::remove_file(&installer);
    r
}

fn base_python(scratch_dir: &Path) -> Result<PathBuf> {
    if let Some(p) = find_python() {
        return Ok(p);
    }
    #[cfg(windows)]
    {
        info!("no system Python 3.10/3.11 found — installing Python {PYTHON_VERSION}…");
        install_python(scratch_dir)?;
        if let Some(p) = find_python() {
            return Ok(p);
        }
        bail!("Python {PYTHON_VERSION} installed but no interpreter was found afterward");
    }
    #[cfg(not(windows))]
    {
        let _ = scratch_dir;
        bail!("no system Python 3.10/3.11 found; install Python and re-run `ufoagent bootstrap`")
    }
}

/// Run a child process, streaming every stdout/stderr line through the logger — so progress is
/// visible everywhere bootstrap runs: the setup console, the tray-spawned window, and the agent
/// log that CI tails live (every pip package, not just "installing requirements…"). Piping also
/// makes pip drop its tty progress bars, keeping the output line-oriented.
fn run(mut cmd: Command, what: &str) -> Result<()> {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().with_context(|| format!("spawning {what}"))?;

    let stderr = child.stderr.take();
    let tag = what.to_string();
    let err_thread = std::thread::spawn(move || {
        if let Some(s) = stderr {
            for line in BufReader::new(s).lines().map_while(|l| l.ok()) {
                if !line.trim().is_empty() {
                    info!("{tag}: {line}");
                }
            }
        }
    });
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(|l| l.ok()) {
            if !line.trim().is_empty() {
                info!("{what}: {line}");
            }
        }
    }
    let _ = err_thread.join();
    let st = child
        .wait()
        .with_context(|| format!("waiting for {what}"))?;
    if !st.success() {
        bail!("{what} failed (exit {:?})", st.code());
    }
    Ok(())
}

/// Provision UFO2. Idempotent. Returns (ufo_home, venv_python). Records the install lifecycle in the
/// env marker (`installing` → `ready`/`broken`) so the dashboard chips and the `run_task` gate track
/// real state — including for a bootstrap launched as its own process (installer `nowait`, the tray's
/// Repair), which the daemon picks up by re-reading the marker each tick.
pub fn bootstrap(ufo_home: Option<String>, git_ref: &str) -> Result<(PathBuf, PathBuf)> {
    use crate::env::{self, EnvState};
    env::set_state(
        env::UFO2,
        EnvState::Installing,
        Some("starting"),
        Some(git_ref),
    );
    match bootstrap_inner(ufo_home, git_ref) {
        Ok(paths) => {
            env::set_state(env::UFO2, EnvState::Ready, None, Some(git_ref));
            Ok(paths)
        }
        Err(e) => {
            env::set_state(
                env::UFO2,
                EnvState::Broken,
                Some(&format!("{e:#}")),
                Some(git_ref),
            );
            Err(e)
        }
    }
}

fn bootstrap_inner(ufo_home: Option<String>, git_ref: &str) -> Result<(PathBuf, PathBuf)> {
    use crate::env::{self, EnvState};
    let phase =
        |detail: &str| env::set_state(env::UFO2, EnvState::Installing, Some(detail), Some(git_ref));

    let cfg = Config::load();
    let home = ufo_home
        .map(PathBuf::from)
        .or_else(|| cfg.ufo_home.clone().map(PathBuf::from))
        .unwrap_or_else(|| cfg.ufo_home_path());
    if let Some(p) = home.parent() {
        std::fs::create_dir_all(p)?;
    }

    if !home.join("requirements.txt").exists() {
        phase("downloading UFO2 source");
        fetch_ufo(&home, git_ref)?;
    }

    let vpy = venv_python(&home);
    if !vpy.exists() {
        phase("creating virtualenv");
        let scratch = home.parent().unwrap_or_else(|| Path::new("."));
        let base = base_python(scratch)?;
        info!("creating venv (base: {})", base.display());
        let mut c = Command::new(base);
        c.arg("-m").arg("venv").arg(home.join(".venv"));
        run(c, "venv create")?;
    }

    phase("installing requirements (torch, etc.)");
    let mut req = std::fs::read_to_string(home.join("requirements.txt"))?;
    for (bad, good) in PIN_OVERRIDES {
        req = req.replace(bad, good);
    }
    let patched = home.join("requirements.ufoagent.txt");
    std::fs::write(&patched, &req)?;

    info!("installing UFO2 requirements (large — torch etc.)…");
    let mut up = Command::new(&vpy);
    up.args([
        "-m",
        "pip",
        "install",
        "--upgrade",
        "pip",
        "setuptools",
        "wheel",
    ]);
    run(up, "pip upgrade")?;
    let mut pi = Command::new(&vpy);
    pi.args(["-m", "pip", "install", "-r"]).arg(&patched);
    run(pi, "pip install")?;

    let mut cfg = Config::load();
    cfg.ufo_home = Some(home.to_string_lossy().to_string());
    cfg.python = Some(vpy.to_string_lossy().to_string());
    cfg.save()?;
    info!(
        "UFO2 provisioned: ufo_home={} python={}",
        home.display(),
        vpy.display()
    );
    Ok((home, vpy))
}
