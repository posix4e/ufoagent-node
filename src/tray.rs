//! System-tray manager UI (Windows). Status + Link/Repair/Run-task/View-log/Dashboard.
//! Non-Windows builds get a stub so the crate checks on macOS/Linux.

use anyhow::Result;

#[cfg(not(windows))]
pub fn run(_version: &str) -> Result<()> {
    anyhow::bail!("the tray manager is only available on Windows")
}

#[cfg(windows)]
pub fn run(version: &str) -> Result<()> {
    imp::run(version)
}

#[cfg(windows)]
mod imp {
    use anyhow::Result;
    use std::time::Duration;
    use tao::event::Event;
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIconBuilder};

    use crate::{status, taskqueue};

    const DASHBOARD: &str = "https://app.ufoagent.xyz";

    enum Ev {
        Menu(MenuEvent),
        Tick,
    }

    fn make_icon() -> Icon {
        // 32x32 magenta disc on transparent — no bundled .ico needed.
        let (w, h) = (32u32, 32u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let (cx, cy, r) = (16.0f32, 16.0f32, 13.0f32);
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                if dx * dx + dy * dy <= r * r {
                    let i = ((y * w + x) * 4) as usize;
                    rgba[i] = 0xff;
                    rgba[i + 1] = 0x2d;
                    rgba[i + 2] = 0x87;
                    rgba[i + 3] = 0xff;
                }
            }
        }
        Icon::from_rgba(rgba, w, h).expect("build tray icon")
    }

    fn exe() -> std::path::PathBuf {
        std::env::current_exe().unwrap_or_else(|_| "ufoagent.exe".into())
    }

    /// Launch a file/exe via the shell with an explicit visible window. The tray FreeConsole()s at
    /// startup, so a console child spawned with std::process::Command (no CREATE_NEW_CONSOLE) gets
    /// NO console and never appears — which made every menu action silently do nothing. ShellExecuteW
    /// creates a proper visible window/console regardless of the caller's console state, and returns
    /// a code we log instead of swallowing (HINSTANCE > 32 = success).
    fn shell_open_show(file: &str, params: Option<&str>) {
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let op = HSTRING::from("open");
        let file_h = HSTRING::from(file);
        let params_h = params.map(HSTRING::from);
        let params_pcwstr = params_h
            .as_ref()
            .map(|h| PCWSTR(h.as_ptr()))
            .unwrap_or(PCWSTR::null());
        let r = unsafe {
            ShellExecuteW(
                HWND::default(),
                &op,
                &file_h,
                params_pcwstr,
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if r.0 as usize <= 32 {
            log::warn!(
                "tray: ShellExecute '{file}' failed (HINSTANCE {})",
                r.0 as usize
            );
        } else {
            log::info!("tray: launched {file}");
        }
    }

    /// Launch `ufoagent <args…>` in its own visible console (Link/Repair). Joins args into a single
    /// parameter string for ShellExecuteW.
    fn spawn_console(args: &[&str]) {
        shell_open_show(&exe().to_string_lossy(), Some(&args.join(" ")));
    }

    /// A native message dialog (title + body + OK). Used to surface the activity recap as a real
    /// GUI window rather than a console — callable from any thread (it runs its own modal loop).
    fn message_box(title: &str, body: &str) {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
        let title = HSTRING::from(title);
        let body = HSTRING::from(body);
        unsafe {
            MessageBoxW(HWND::default(), &body, &title, MB_OK | MB_ICONINFORMATION);
        }
    }

    /// Run one queued task in this interactive session: `ufoagent run --task <task> -r <request>`.
    /// Captures output to tasks\logs\<id>.txt and returns the exit + a tail as the result.
    fn run_one(req: &taskqueue::TaskRequest) -> taskqueue::TaskResult {
        log::info!("tray: running task {} ({})", req.id, req.task);
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = std::process::Command::new(exe());
        cmd.args(["run", "--task", &req.task]);
        if let Some(r) = &req.request {
            cmd.arg("-r").arg(r);
        }
        // The service already records this as a remote command; tell `run` not to double-log it.
        cmd.env("UFOAGENT_FROM_QUEUE", "1");
        // No flashing console on the user's desktop while UFO2 drives the GUI — run it headless.
        cmd.creation_flags(CREATE_NO_WINDOW);
        let out = match cmd.output() {
            Ok(o) => o,
            Err(e) => {
                return taskqueue::TaskResult {
                    id: req.id.clone(),
                    status: "failed".into(),
                    result: format!("could not launch UFO2: {e}"),
                }
            }
        };

        // Persist the full transcript for diagnostics; the result carries a tail.
        let mut transcript = String::from_utf8_lossy(&out.stdout).into_owned();
        transcript.push_str(&String::from_utf8_lossy(&out.stderr));
        let logs = crate::config::config_dir().join("tasks").join("logs");
        if std::fs::create_dir_all(&logs).is_ok() {
            let _ = std::fs::write(logs.join(format!("{}.txt", req.id)), &transcript);
        }

        let tail: String = transcript
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        taskqueue::TaskResult {
            id: req.id.clone(),
            status: if out.status.success() {
                "done".into()
            } else {
                "failed".into()
            },
            result: if tail.is_empty() {
                format!("exit {:?}", out.status.code())
            } else {
                format!("exit {:?}\n{tail}", out.status.code())
            },
        }
    }

    fn status_line() -> String {
        let s = status::load();
        format!(
            "UFOAgent — {}, {}",
            if s.online { "online" } else { "offline" },
            if s.linked { "linked" } else { "not linked" },
        )
    }

    /// Per-session single-instance guard. The service (re)spawns the tray into the console session
    /// while the installer's Startup-folder shortcut may also launch one — without this they'd both
    /// appear. Holds a session-named mutex for the process lifetime; returns true if another tray
    /// already owns it. On any failure to create the guard, returns false (better to run than not).
    fn another_tray_running() -> bool {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, TRUE};
        use windows::Win32::System::Threading::CreateMutexW;
        let name: Vec<u16> = "UFOAgentTraySingleton\0".encode_utf16().collect();
        unsafe {
            match CreateMutexW(None, TRUE, PCWSTR(name.as_ptr())) {
                Ok(h) => {
                    if GetLastError() == ERROR_ALREADY_EXISTS {
                        let _ = CloseHandle(h);
                        true
                    } else {
                        // Deliberately don't CloseHandle(h): HANDLE has no Drop, so leaving it open
                        // holds the mutex for the whole process lifetime (we own the single slot).
                        false
                    }
                }
                Err(_) => false,
            }
        }
    }

    pub fn run(_version: &str) -> Result<()> {
        if another_tray_running() {
            log::info!("tray: another instance is already running in this session; exiting");
            return Ok(());
        }
        // Drop the console window Windows allocates for this console-subsystem exe when the tray
        // is launched by the installer or the logon task — leave just the 🛸 tray icon.
        unsafe {
            let _ = windows::Win32::System::Console::FreeConsole();
        }
        let event_loop = EventLoopBuilder::<Ev>::with_user_event().build();

        let proxy = event_loop.create_proxy();
        MenuEvent::set_event_handler(Some(move |e| {
            let _ = proxy.send_event(Ev::Menu(e));
        }));

        let menu = Menu::new();
        let m_status = MenuItem::new(status_line(), false, None);
        let m_link = MenuItem::new("Link / Re-link…", true, None);
        let m_repair = MenuItem::new("Repair", true, None);
        let m_run = MenuItem::new("Run a task…", true, None);
        let m_activity = MenuItem::new("What's this node been doing?", true, None);
        let m_log = MenuItem::new("View log", true, None);
        let m_dash = MenuItem::new("Open dashboard", true, None);
        let m_quit = MenuItem::new("Quit", true, None);
        menu.append_items(&[
            &m_status,
            &PredefinedMenuItem::separator(),
            &m_link,
            &m_repair,
            &m_run,
            &m_activity,
            &PredefinedMenuItem::separator(),
            &m_log,
            &m_dash,
            &PredefinedMenuItem::separator(),
            &m_quit,
        ])?;

        let _tray = TrayIconBuilder::new()
            .with_tooltip("UFOAgent")
            .with_icon(make_icon())
            .with_menu(Box::new(menu))
            .build()?;

        // Refresh status every 5s, and touch the liveness marker so the SYSTEM service knows there's
        // a live interactive desktop here to run GUI tasks on.
        let tick = event_loop.create_proxy();
        std::thread::spawn(move || loop {
            let _ = taskqueue::touch_alive();
            std::thread::sleep(Duration::from_secs(5));
            let _ = tick.send_event(Ev::Tick);
        });

        // Task executor: the service (Session 0) drops run_task requests in the file queue; we run
        // them here in the interactive session (where UFO2 can drive the GUI) and report back. Own
        // thread so a long task doesn't stall the status/liveness tick above.
        std::thread::spawn(|| loop {
            if let Some(req) = taskqueue::next_pending() {
                let _ = taskqueue::report(&run_one(&req));
            } else {
                std::thread::sleep(Duration::from_secs(2));
            }
        });

        let (id_link, id_repair, id_run, id_activity, id_log, id_dash, id_quit) = (
            m_link.id().clone(),
            m_repair.id().clone(),
            m_run.id().clone(),
            m_activity.id().clone(),
            m_log.id().clone(),
            m_dash.id().clone(),
            m_quit.id().clone(),
        );
        let log_path = crate::config::config_dir()
            .join("logs")
            .join("ufoagent.log");

        event_loop.run(move |event, _target, control_flow| {
            *control_flow = ControlFlow::Wait;
            match event {
                Event::UserEvent(Ev::Tick) => m_status.set_text(status_line()),
                Event::UserEvent(Ev::Menu(e)) => {
                    if e.id == id_link {
                        log::info!("tray: menu action — link");
                        spawn_console(&["link", "--force", "--pause"]);
                    } else if e.id == id_repair {
                        log::info!("tray: menu action — repair");
                        spawn_console(&["repair"]);
                    } else if e.id == id_run {
                        log::info!("tray: menu action — run a task");
                        let exes = exe().to_string_lossy().to_string();
                        // ShellExecute powershell in its own console (the tray has none) so the
                        // Read-Host prompt is actually visible.
                        let ps = format!(
                            "-NoProfile -Command \"$r = Read-Host 'Task request'; & '{exes}' run --task adhoc -r $r; Read-Host 'done — press Enter'\""
                        );
                        shell_open_show("powershell.exe", Some(&ps));
                    } else if e.id == id_activity {
                        log::info!("tray: menu action — activity summary");
                        // Build the (LLM) recap off the UI thread — it makes a network call — then
                        // pop a native dialog with the result. A GUI window, not a console.
                        std::thread::spawn(|| {
                            let text = crate::activity::summarize();
                            message_box("UFOAgent — what this node has been doing", &text);
                        });
                    } else if e.id == id_log {
                        log::info!("tray: menu action — view log");
                        // notepad.exe + the log as its parameter (`.log` has no default association
                        // on Windows Server). Via ShellExecute so it shows from the console-less tray.
                        shell_open_show("notepad.exe", Some(&log_path.to_string_lossy()));
                    } else if e.id == id_dash {
                        log::info!("tray: menu action — open dashboard");
                        shell_open_show(DASHBOARD, None);
                    } else if e.id == id_quit {
                        *control_flow = ControlFlow::Exit;
                    }
                }
                _ => {}
            }
        });
    }
}
