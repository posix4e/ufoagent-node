//! Windows service: install / uninstall / run the credential-refresh + heartbeat daemon.
//! Non-Windows builds get a stub so the crate compiles on macOS/Linux for `cargo check`/`test`.

use anyhow::Result;

use crate::cli::ServiceAction;

#[cfg(not(windows))]
pub fn run_action(_action: &ServiceAction) -> Result<()> {
    anyhow::bail!("the Windows service is only available on Windows")
}

#[cfg(windows)]
pub fn run_action(action: &ServiceAction) -> Result<()> {
    match action {
        ServiceAction::Install => imp::install(),
        ServiceAction::Uninstall => imp::uninstall(),
        ServiceAction::Run => imp::run(),
    }
}

#[cfg(windows)]
mod imp {
    use anyhow::Result;
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_dispatcher;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    const SERVICE_NAME: &str = "UFOAgent";
    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
    const VERSION: &str = env!("CARGO_PKG_VERSION");

    windows_service::define_windows_service!(ffi_service_main, service_main);

    pub fn run() -> Result<()> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
        Ok(())
    }

    fn service_main(_args: Vec<OsString>) {
        if let Err(e) = run_service() {
            log::error!("service failed: {e}");
        }
    }

    fn status(state: ServiceState, accept: ServiceControlAccept) -> ServiceStatus {
        ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        }
    }

    fn run_service() -> Result<()> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_handler = stop.clone();
        let handler = move |control: ServiceControl| -> ServiceControlHandlerResult {
            match control {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    stop_handler.store(true, Ordering::SeqCst);
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };
        let handle = service_control_handler::register(SERVICE_NAME, handler)?;
        handle.set_service_status(status(
            ServiceState::Running,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        ))?;

        let _ = crate::daemon::run_daemon(VERSION, move || stop.load(Ordering::SeqCst));

        handle.set_service_status(status(ServiceState::Stopped, ServiceControlAccept::empty()))?;
        Ok(())
    }

    pub fn install() -> Result<()> {
        let manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)?;
        let info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from("UFOAgent"),
            service_type: SERVICE_TYPE,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: std::env::current_exe()?,
            launch_arguments: vec![OsString::from("service"), OsString::from("run")],
            dependencies: vec![],
            account_name: None,
            account_password: None,
        };
        let service =
            manager.create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)?;
        let _ = service.set_description("UFOAgent node daemon — keeps LLM credentials fresh");
        service.start::<OsString>(&[])?;
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(
            SERVICE_NAME,
            ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
        )?;
        let _ = service.stop();
        service.delete()?;
        Ok(())
    }
}
