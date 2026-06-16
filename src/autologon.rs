//! Unattended-node auto-logon (Windows): configure the box to auto-log a user into a live,
//! unlocked console session so UFO2 has a desktop to drive when nobody signs in.
//!
//! v1 stores DefaultPassword in the registry. TODO: store it as an LSA secret
//! (LsaStorePrivateData "DefaultPassword") like Sysinternals Autologon, for at-rest encryption.

use anyhow::Result;

#[cfg(not(windows))]
pub fn run(
    _user: &str,
    _password: Option<&str>,
    _domain: Option<&str>,
    _disable: bool,
) -> Result<()> {
    anyhow::bail!("autologon is only available on Windows")
}

#[cfg(windows)]
pub fn run(user: &str, password: Option<&str>, domain: Option<&str>, disable: bool) -> Result<()> {
    imp::run(user, password, domain, disable)
}

#[cfg(windows)]
mod imp {
    use anyhow::{anyhow, Result};
    use winreg::enums::*;
    use winreg::RegKey;

    const WINLOGON: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon";

    pub fn run(
        user: &str,
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

        let pw = password.ok_or_else(|| anyhow!("--password is required to enable autologon"))?;
        winlogon.set_value("AutoAdminLogon", &"1")?;
        winlogon.set_value("DefaultUserName", &user)?;
        winlogon.set_value("DefaultDomainName", &domain.unwrap_or("."))?;
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
            "autologon enabled for {user} — NOTE: password is stored in the registry; \
             use only on dedicated, isolated agent machines."
        );
        Ok(())
    }
}
