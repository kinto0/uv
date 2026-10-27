use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::process::Command;

use uv_command_support::{ExitStatus, Printer, child::run_to_completion};
use uv_configuration::ColorChoice;
use uv_normalize::PackageName;
use uv_settings::{FrozenSource, LockCheck};
use uv_shell::shlex_posix;
use uv_static::EnvVars;

/// Run a type check powered by Pyrefly.
///
/// Pyrefly's `uv` build system reads members, environments, and Python versions from
/// `uv workspace metadata`. uv selects workspace members as build system targets, or files when
/// there are no members to select, and points the build system at the selected environment and
/// lockfile settings, in the same way as for ty.
pub(super) async fn run(
    pyrefly_path: &Path,
    target_dir: &Path,
    members: &[PackageName],
    files: &[PathBuf],
    venv_path: Option<&Path>,
    lock_check: LockCheck,
    frozen: Option<FrozenSource>,
    show_version: bool,
    show_command: bool,
    color: ColorChoice,
    printer: Printer,
) -> Result<ExitStatus> {
    if show_version {
        let output = Command::new(pyrefly_path)
            .arg("--version")
            .output()
            .await
            .context("Failed to query Pyrefly version")?;
        if !output.status.success() {
            anyhow::bail!("Failed to query Pyrefly version");
        }
        writeln!(
            printer.stderr(),
            "Using {}",
            String::from_utf8_lossy(&output.stdout).trim()
        )?;
    }

    let mut command = Command::new(pyrefly_path);
    command.current_dir(target_dir);
    command.arg("check");
    command.arg("--color").arg(color.as_str());
    // The build system runs `uv workspace metadata --active`, so `VIRTUAL_ENV` selects the
    // environment, and `UV` selects this uv executable.
    if let Some(venv_path) = venv_path {
        command.env(EnvVars::VIRTUAL_ENV, venv_path);
    }
    if let Ok(uv) = std::env::current_exe() {
        command.env(EnvVars::UV, uv);
    }
    if frozen.is_some() {
        command.env(EnvVars::UV_FROZEN, "1");
    } else {
        command.env_remove(EnvVars::UV_FROZEN);
    }
    match lock_check {
        LockCheck::Enabled(_) => command.env(EnvVars::UV_LOCKED, "1"),
        LockCheck::Disabled => command.env_remove(EnvVars::UV_LOCKED),
    };
    for member in members {
        command.arg("--target").arg(format!("//member:{member}"));
    }
    if !files.is_empty() {
        command.arg("--");
        for file in files {
            command.arg(file.strip_prefix(target_dir).unwrap_or(file));
        }
    }

    if show_command {
        let mut stderr = printer.stderr_important();
        write!(stderr, "Running `pyrefly")?;
        for argument in command.as_std().get_args() {
            write!(stderr, " {}", shlex_posix(argument))?;
        }
        writeln!(stderr, "`")?;
    }

    let handle = command.spawn().context("Failed to spawn `pyrefly check`")?;
    run_to_completion(handle).await
}
