use std::{
    collections::HashMap,
    env::{self, VarError},
    io,
    process::{Command, Stdio},
};

use bevy::app::{App, AppExit};

const APP_PROCESS_ENV: &str = "ROUNDO_APP_PROCESS";
const SPAWN_STAGE: &str = "spawn";
const RUN_STAGE: &str = "run";

/// A registered App configuration, executed only in the spawned process.
pub(crate) struct AppProcess {
    pub(crate) id: &'static str,
    pub(crate) configure: fn(&mut App),
}

impl AppProcess {
    /// Waits only for the bootstrap process to report whether it created the App process.
    pub(crate) fn spawn(&self) -> io::Result<()> {
        let status = self.command(SPAWN_STAGE)?.status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "App process bootstrap failed ({status})"
            )));
        }
        Ok(())
    }

    fn command(&self, stage: &str) -> io::Result<Command> {
        let mut command = Command::new(env::current_exe()?);
        command
            .env(APP_PROCESS_ENV, format!("{stage}:{}", self.id))
            .stdin(Stdio::null());
        Ok(command)
    }
}

/// Selects the launcher, the short-lived bootstrap, or the requested App.
pub(crate) fn run<'a>(
    processes: impl IntoIterator<Item = &'a AppProcess>,
    run_launcher: impl FnOnce() -> AppExit,
) -> AppExit {
    let mut registry = HashMap::new();
    for process in processes {
        if process.id.is_empty() || process.id.contains('\0') {
            eprintln!("Invalid App process ID: {:?}", process.id);
            return AppExit::error();
        }
        if registry.insert(process.id, process).is_some() {
            eprintln!("Duplicate App process ID: {:?}", process.id);
            return AppExit::error();
        }
    }

    let request = match env::var(APP_PROCESS_ENV) {
        Ok(request) => request,
        Err(VarError::NotPresent) => return run_launcher(),
        Err(error) => {
            eprintln!("Could not read App process request: {error}");
            return AppExit::error();
        }
    };
    let Some((stage @ (SPAWN_STAGE | RUN_STAGE), id)) = request.split_once(':') else {
        eprintln!("Invalid App process request: {request:?}");
        return AppExit::error();
    };
    let Some(process) = registry.get(id) else {
        eprintln!("Unknown App process ID: {id:?}");
        return AppExit::error();
    };
    if stage == SPAWN_STAGE {
        // Exit as soon as the App process has been created. The launcher never owns its handle.
        return match process.command(RUN_STAGE).and_then(|mut cmd| cmd.spawn()) {
            Ok(_) => AppExit::Success,
            Err(error) => {
                eprintln!("Could not create App process {id:?}: {error}");
                AppExit::error()
            }
        };
    }

    let mut app = App::new();
    (process.configure)(&mut app);
    app.run()
}
