//! Provision Microsoft UFO2 into a managed home: download source zip + venv + install requirements.
//! No external tools required — fetches the GitHub source archive over HTTP and unpacks it with
//! Windows' built-in tar, auto-installs Python if none is present, and uses a pinned `uv` (Astral) to
//! build the venv + install faster than pip (falling back to pip if uv can't be fetched). UFO2 stays
//! Python; this just sets it up and records ufo_home + the venv python.

use anyhow::{anyhow, bail, Context, Result};
use log::info;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::controlplane::USER_AGENT;
use crate::{config::Config, ufo_config};

/// UFO2 pins some packages with no Python 3.11 wheel (build-from-source fails on modern
/// setuptools). Rewrite those to wheel-backed versions.
const PIN_OVERRIDES: &[(&str, &str)] = &[("pandas==1.4.3", "pandas==1.5.3")];

/// GitHub source-archive endpoint. No `git` needed — we download the branch zip and unpack it
/// with the `tar.exe` shipped in Windows (libarchive, reads zips).
const UFO_ARCHIVE_BASE: &str = "https://github.com/microsoft/UFO/archive/refs/heads";

/// Pinned CPython used when no system interpreter is found (UFO2 is Python; can't drop it).
#[cfg(windows)]
const PYTHON_VERSION: &str = "3.11.9";

/// Pinned `uv` (Astral's installer). Builds the venv + installs requirements faster than pip: the
/// UFO2 dep tree (torch/scipy/transformers…) benefits from uv's parallel installer. We ask uv to
/// compile bytecode so UFO startup pays less import cost later. Best-effort — provisioning falls back
/// to pip if the binary can't be fetched.
#[cfg(windows)]
const UV_VERSION: &str = "0.11.21";

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

/// Fetch the pinned standalone `uv` into `scratch` and return the path to `uv.exe`. The Windows release
/// zip holds uv.exe/uvw.exe/uvx.exe at the top level — unpack straight in with Windows' `tar`. No Python
/// needed. Best-effort: the caller falls back to pip if this errs (offline, proxy, GitHub block, …).
#[cfg(windows)]
fn ensure_uv(scratch: &Path) -> Result<PathBuf> {
    let uv = scratch.join("uv.exe");
    if uv.is_file() {
        return Ok(uv);
    }
    let url = format!(
        "https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/uv-x86_64-pc-windows-msvc.zip"
    );
    let zip = scratch.join("uv.zip");
    info!("downloading uv {UV_VERSION} (fast installer)…");
    download(&url, &zip)?;
    let mut c = Command::new("tar");
    c.arg("-xf").arg(&zip).arg("-C").arg(scratch);
    let r = run(c, "extract uv");
    let _ = std::fs::remove_file(&zip);
    r?;
    if !uv.is_file() {
        bail!(
            "uv archive extracted but uv.exe is missing under {}",
            scratch.display()
        );
    }
    Ok(uv)
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

/// pip installs pywin32 (UFO2 pulls it in via pywinauto) but never runs pywin32's post-install
/// step — so its loader DLLs in `site-packages\pywin32_system32` (`pywintypes3XX.dll`,
/// `pythoncom3XX.dll`) aren't on the DLL search path. Running the post-install copies them to
/// System32. (Note: pywin32 3.12 does NOT bundle the MFC runtime `mfc140u.dll` that `win32ui` also
/// needs — that comes from the VC++ redistributable, installed separately in `install_vc_redist`.)
///
/// Best-effort: the script can exit nonzero or warn even on success, and `verify_ufo_imports` is the
/// real readiness verdict — so a hiccup here is logged, not fatal. No-op when the script is absent.
fn post_install_pywin32(vpy: &Path) {
    let script = match vpy.parent() {
        Some(scripts) => scripts.join("pywin32_postinstall.py"),
        None => return,
    };
    if !script.exists() {
        return;
    }
    info!("running pywin32 post-install (fixes win32ui DLL load)…");
    let mut c = Command::new(vpy);
    c.arg(&script).arg("-install");
    if let Err(e) = run(c, "pywin32 post-install") {
        log::warn!("pywin32 post-install did not complete cleanly: {e}");
    }
}

/// `win32ui` (pulled in via pywinauto) is linked against the MFC runtime `mfc140u.dll`, which pywin32
/// 3.12 no longer bundles and a fresh Windows Server lacks (CI runners have it via VS, masking the
/// bug). Without it `import win32ui` dies with "DLL load failed" and every UFO2 task crashes. Install
/// the official VC++ redistributable, which drops mfc140u.dll into System32. Idempotent: a newer
/// redist already present exits 1638, a successful install needing reboot exits 3010 — both fine.
#[cfg(windows)]
fn install_vc_redist(scratch_dir: &Path) -> Result<()> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
    let mfc = Path::new(&windir).join("System32").join("mfc140u.dll");
    // Already there (the common case on machines with the redist) — nothing to do.
    if mfc.exists() {
        return Ok(());
    }
    let url = "https://aka.ms/vs/17/release/vc_redist.x64.exe";
    let exe = scratch_dir.join("vc_redist.x64.exe");
    info!("installing the Visual C++ runtime (mfc140u.dll, for win32ui)…");
    download(url, &exe)?;
    let run_vc = |mode: &str| -> std::io::Result<Option<i32>> {
        Command::new(&exe)
            .args([mode, "/quiet", "/norestart"])
            .status()
            .map(|s| s.code())
    };
    let code = run_vc("/install").with_context(|| "running vc_redist /install")?;
    // If the redist is registered but mfc140u.dll is gone (a corrupt/partial runtime), /install
    // no-ops (1638) and won't restore the file — try a repair. MSI repair is keypath-based, so it's
    // not guaranteed to restore a single missing file; best-effort.
    if !mfc.exists() {
        info!("mfc140u.dll still missing after /install (code {code:?}); repairing VC++ runtime…");
        let _ = run_vc("/repair").with_context(|| "running vc_redist /repair")?;
    }
    let _ = std::fs::remove_file(&exe);
    // Best-effort — don't fail bootstrap here. On a real fresh VM (the actual scenario) the redist
    // isn't registered, so /install installs it cleanly; `verify_ufo_imports` is the authoritative
    // readiness gate that marks the env broken if UFO2 still can't import.
    if !mfc.exists() {
        log::warn!("VC++ runtime install did not yield mfc140u.dll (install exit {code:?})");
    }
    Ok(())
}
#[cfg(not(windows))]
fn install_vc_redist(_scratch_dir: &Path) -> Result<()> {
    Ok(())
}

/// The real readiness verdict: pip succeeding isn't enough — UFO2's GUI stack must actually import
/// and its native MCP servers must register. If these fail the env is NOT ready, so bootstrap returns
/// Err and the marker becomes `broken` rather than a "ready" chip whose tasks crash.
fn verify_ufo_imports(vpy: &Path, home: &Path) -> Result<()> {
    info!("verifying UFO2 imports and MCP registry (win32ui, pywinauto)…");
    let out = Command::new(vpy)
        .current_dir(home)
        .env("PYTHONPATH", home)
        .args([
            "-c",
            r#"import win32ui, pywinauto
import ufo.client.mcp.local_servers.ui_mcp_server
import ufo.client.mcp.local_servers.cli_mcp_server
from ufo.client.mcp.mcp_registry import MCPRegistry
required = {"HostUIExecutor", "AppUIExecutor", "UICollector", "CommandLineExecutor"}
registered = set(MCPRegistry.list())
missing = sorted(required - registered)
assert not missing, f"missing MCP server registrations: {missing}; registered={sorted(registered)}"
"#,
        ])
        .output()
        .with_context(|| "running UFO2 import check")?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let last = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("import failed")
        .trim();
    bail!("UFO2 import check failed: {last}");
}

/// Provision UFO2. Idempotent. Returns (ufo_home, venv_python). Records the install lifecycle in the
/// env marker (`installing` → `ready`/`broken`) so the dashboard chips and the `run_task` gate track
/// real state — including for a bootstrap launched as its own process (installer `nowait`, the tray's
/// Repair), which the login agent picks up by re-reading the marker each tick.
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
    phase("configuring UFO2 unattended mode");
    ufo_config::apply_unattended_mode(&home)?;

    let vpy = venv_python(&home);
    if vpy.exists() {
        let mut cfg = Config::load();
        cfg.ufo_home = Some(home.to_string_lossy().to_string());
        cfg.python = Some(vpy.to_string_lossy().to_string());
        cfg.save()?;

        phase("using provisioned UFO2 snapshot");
        phase("verifying UFO2 imports");
        match verify_ufo_imports(&vpy, &home) {
            Ok(()) => {
                info!(
                    "UFO2 already provisioned: ufo_home={} python={}",
                    home.display(),
                    vpy.display()
                );
                return Ok((home, vpy));
            }
            Err(e) => {
                log::warn!("existing UFO2 install failed readiness check; repairing deps: {e:#}");
            }
        }
    }

    let scratch = home
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    // Prefer uv (Astral) for the venv + install — ~8× faster than pip on the UFO2 tree (parallel
    // install, no byte-compile). Windows-only + best-effort; `None` falls back to the pip path below.
    #[cfg(windows)]
    let uv = ensure_uv(&scratch)
        .map_err(|e| log::warn!("uv unavailable, falling back to pip: {e:#}"))
        .ok();
    #[cfg(not(windows))]
    let uv: Option<PathBuf> = None;

    if !vpy.exists() {
        phase("creating virtualenv");
        let base = base_python(&scratch)?;
        info!("creating venv (base: {})", base.display());
        match &uv {
            Some(uv) => {
                let mut c = Command::new(uv);
                c.arg("venv")
                    .arg("--python")
                    .arg(&base)
                    .arg(home.join(".venv"));
                run(c, "uv venv")?;
            }
            None => {
                let mut c = Command::new(&base);
                c.arg("-m").arg("venv").arg(home.join(".venv"));
                run(c, "venv create")?;
            }
        }
    }

    phase("installing requirements (torch, etc.)");
    let mut req = std::fs::read_to_string(home.join("requirements.txt"))?;
    for (bad, good) in PIN_OVERRIDES {
        req = req.replace(bad, good);
    }
    let patched = home.join("requirements.ufoagent.txt");
    std::fs::write(&patched, &req)?;

    info!("installing UFO2 requirements (large — torch etc.)…");
    match &uv {
        Some(uv) => {
            let mut pi = Command::new(uv);
            pi.args(["pip", "install", "--python"])
                .arg(&vpy)
                .arg("--compile-bytecode")
                .arg("-r")
                .arg(&patched);
            run(pi, "uv pip install")?;
        }
        None => {
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
        }
    }

    post_install_pywin32(&vpy);

    // MFC runtime for win32ui — a fresh Windows lacks it and pywin32 doesn't ship it. Best-effort:
    // if it fails, the import check below is the authoritative gate.
    phase("installing Visual C++ runtime");
    let scratch = home.parent().unwrap_or_else(|| Path::new("."));
    if let Err(e) = install_vc_redist(scratch) {
        log::warn!("VC++ runtime install did not complete cleanly: {e}");
    }

    let mut cfg = Config::load();
    cfg.ufo_home = Some(home.to_string_lossy().to_string());
    cfg.python = Some(vpy.to_string_lossy().to_string());
    cfg.save()?;

    // Authoritative readiness gate: UFO2's GUI imports must actually load. A failure here (e.g. the
    // win32ui/MFC class of bug) returns Err → the marker becomes `broken`, not a ready chip whose
    // tasks crash on the first run.
    phase("verifying UFO2 imports");
    verify_ufo_imports(&vpy, &home)?;

    info!(
        "UFO2 provisioned: ufo_home={} python={}",
        home.display(),
        vpy.display()
    );
    Ok((home, vpy))
}
