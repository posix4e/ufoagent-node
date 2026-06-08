//! Provision Microsoft UFO2 into a managed home: git clone + venv + pip install.
//! UFO2 stays Python; this just sets it up and records ufo_home + the venv python.

use anyhow::{bail, Context, Result};
use log::info;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;

/// UFO2 pins some packages with no Python 3.11 wheel (build-from-source fails on modern
/// setuptools). Rewrite those to wheel-backed versions.
const PIN_OVERRIDES: &[(&str, &str)] = &[("pandas==1.4.3", "pandas==1.5.3")];

const UFO_GIT: &str = "https://github.com/microsoft/UFO.git";

fn which(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|d| d.join(&exe))
        .find(|p| p.is_file())
}

fn venv_python(home: &Path) -> PathBuf {
    if cfg!(windows) {
        home.join(".venv").join("Scripts").join("python.exe")
    } else {
        home.join(".venv").join("bin").join("python")
    }
}

fn base_python() -> Result<PathBuf> {
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
                    return Ok(PathBuf::from(s));
                }
            }
        }
    }
    bail!("no system Python 3.10/3.11 found; install Python and re-run `ufoagent bootstrap`")
}

fn run(mut cmd: Command, what: &str) -> Result<()> {
    let st = cmd.status().with_context(|| format!("spawning {what}"))?;
    if !st.success() {
        bail!("{what} failed (exit {:?})", st.code());
    }
    Ok(())
}

/// Provision UFO2. Idempotent. Returns (ufo_home, venv_python).
pub fn bootstrap(ufo_home: Option<String>, git_ref: &str) -> Result<(PathBuf, PathBuf)> {
    let cfg = Config::load();
    let home = ufo_home
        .map(PathBuf::from)
        .or_else(|| cfg.ufo_home.clone().map(PathBuf::from))
        .unwrap_or_else(|| cfg.ufo_home_path());
    if let Some(p) = home.parent() {
        std::fs::create_dir_all(p)?;
    }

    if !home.join("requirements.txt").exists() {
        let git = which("git").context("git not found; install Git and retry")?;
        info!("git clone microsoft/UFO -> {}", home.display());
        let mut c = Command::new(git);
        c.args(["clone", "--depth", "1", "--branch", git_ref, UFO_GIT])
            .arg(&home);
        run(c, "git clone")?;
    }

    let vpy = venv_python(&home);
    if !vpy.exists() {
        let base = base_python()?;
        info!("creating venv (base: {})", base.display());
        let mut c = Command::new(base);
        c.arg("-m").arg("venv").arg(home.join(".venv"));
        run(c, "venv create")?;
    }

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
