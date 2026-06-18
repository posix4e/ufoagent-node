//! Unattended-node auto-logon (Windows): configure the box to auto-log a user into a live,
//! unlocked console session so UFO2 has a desktop to drive when nobody signs in.
//!
//! v1 stores DefaultPassword in the registry. TODO: store it as an LSA secret
//! (LsaStorePrivateData "DefaultPassword") like Sysinternals Autologon, for at-rest encryption.

use anyhow::Result;

#[cfg(not(windows))]
pub fn run(
    _user: Option<&str>,
    _password: Option<&str>,
    _domain: Option<&str>,
    _disable: bool,
) -> Result<()> {
    anyhow::bail!("autologon is only available on Windows")
}

#[cfg(windows)]
pub fn run(
    user: Option<&str>,
    password: Option<&str>,
    domain: Option<&str>,
    disable: bool,
) -> Result<()> {
    imp::run(user, password, domain, disable)
}

#[cfg(windows)]
mod imp {
    use anyhow::{Context, Result};
    use std::io::{self, Write};
    use winreg::enums::*;
    use winreg::RegKey;

    const WINLOGON: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon";

    pub fn run(
        user: Option<&str>,
        password: Option<&str>,
        domain: Option<&str>,
        disable: bool,
    ) -> Result<()> {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let winlogon = hklm.open_subkey_with_flags(WINLOGON, KEY_SET_VALUE | KEY_QUERY_VALUE)?;

        if disable {
            winlogon.set_value("AutoAdminLogon", &"0")?;
            let _ = winlogon.delete_value("DefaultPassword");
            println!("autologon disabled");
            return Ok(());
        }

        println!();
        println!("UFOAgent unattended GUI mode");
        println!("This signs Windows into an unlocked desktop after reboot so GUI tasks can run.");
        println!("Use a dedicated, low-privilege account on an isolated agent machine.");
        println!();

        let interactive = user.is_none() || password.is_none();
        let default_user = current_env("USERNAME").unwrap_or_else(|| "ufoagent".to_string());
        let default_domain = domain
            .map(str::to_string)
            .or_else(|| current_env("USERDOMAIN"))
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| ".".to_string());

        let user = match user {
            Some(u) => u.trim().to_string(),
            None => prompt_default("Windows user", &default_user)?,
        };
        if user.is_empty() {
            anyhow::bail!("Windows user is required");
        }

        let domain = match domain {
            Some(d) => d.trim().to_string(),
            None if interactive => prompt_default("Domain/computer", &default_domain)?,
            None => default_domain,
        };

        let pw = match password {
            Some(p) => p.to_string(),
            None => read_password("Password: ")?,
        };
        if pw.is_empty() {
            anyhow::bail!("password is required to enable autologon");
        }

        if interactive && !confirm_default_yes(&format!("Enable auto-logon for {domain}\\{user}?"))?
        {
            println!("autologon unchanged");
            return Ok(());
        }

        winlogon.set_value("AutoAdminLogon", &"1")?;
        winlogon.set_value("DefaultUserName", &user)?;
        winlogon.set_value("DefaultDomainName", &domain)?;
        winlogon.set_value("DefaultPassword", &pw)?;

        // Keep the console session active + unlocked so UFO2 can drive it.
        let perso = hklm
            .create_subkey(r"SOFTWARE\Policies\Microsoft\Windows\Personalization")?
            .0;
        perso.set_value("NoLockScreen", &1u32)?;

        // Suppress the first-logon "privacy settings" OOBE page. On a fresh auto-logon session it
        // pops up modally and STEALS FOREGROUND — blocking UFO2 (or any GUI automation) from driving
        // the desktop until a human clicks Accept. An unattended node must never wait on that.
        let oobe = hklm
            .create_subkey(r"SOFTWARE\Policies\Microsoft\Windows\OOBE")?
            .0;
        oobe.set_value("DisablePrivacyExperience", &1u32)?;

        println!(
            "autologon enabled for {domain}\\{user} — NOTE: password is stored in the registry; \
             use only on dedicated, isolated agent machines."
        );
        Ok(())
    }

    fn current_env(name: &str) -> Option<String> {
        std::env::var(name)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn prompt_default(label: &str, default: &str) -> Result<String> {
        print!("{label} [{default}]: ");
        io::stdout().flush().context("flush stdout")?;
        let mut line = String::new();
        io::stdin().read_line(&mut line).context("read stdin")?;
        let value = line.trim();
        Ok(if value.is_empty() {
            default.to_string()
        } else {
            value.to_string()
        })
    }

    fn confirm_default_yes(prompt: &str) -> Result<bool> {
        print!("{prompt} [Y/n]: ");
        io::stdout().flush().context("flush stdout")?;
        let mut line = String::new();
        io::stdin().read_line(&mut line).context("read stdin")?;
        let value = line.trim();
        Ok(
            value.is_empty()
                || value.eq_ignore_ascii_case("y")
                || value.eq_ignore_ascii_case("yes"),
        )
    }

    fn read_password(prompt: &str) -> Result<String> {
        print!("{prompt}");
        io::stdout().flush().context("flush stdout")?;
        let restore = ConsoleEcho::disable();
        let mut line = String::new();
        let read = io::stdin().read_line(&mut line).context("read stdin");
        drop(restore);
        println!();
        read?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }

    struct ConsoleEcho {
        handle: windows::Win32::Foundation::HANDLE,
        mode: windows::Win32::System::Console::CONSOLE_MODE,
    }

    impl ConsoleEcho {
        fn disable() -> Option<Self> {
            use windows::Win32::System::Console::{
                GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_ECHO_INPUT, STD_INPUT_HANDLE,
            };

            unsafe {
                let handle = GetStdHandle(STD_INPUT_HANDLE).ok()?;
                let mut mode = windows::Win32::System::Console::CONSOLE_MODE(0);
                GetConsoleMode(handle, &mut mode).ok()?;
                let next =
                    windows::Win32::System::Console::CONSOLE_MODE(mode.0 & !ENABLE_ECHO_INPUT.0);
                SetConsoleMode(handle, next).ok()?;
                Some(Self { handle, mode })
            }
        }
    }

    impl Drop for ConsoleEcho {
        fn drop(&mut self) {
            unsafe {
                let _ = windows::Win32::System::Console::SetConsoleMode(self.handle, self.mode);
            }
        }
    }
}
