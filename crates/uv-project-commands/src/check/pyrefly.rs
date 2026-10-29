use std::ffi::{OsStr, OsString};
use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::process::Command;

use uv_command_support::{ExitStatus, Printer, child::run_to_completion};
use uv_configuration::ColorChoice;
use uv_pep440::Version;
use uv_python_interpreter::PythonEnvironment;
use uv_settings::{FrozenSource, LockCheck};
use uv_shell::shlex_posix;
use uv_static::EnvVars;

/// Run a type check powered by Pyrefly.
///
/// uv passes the root of each selected workspace member as `--project-dir`, so Pyrefly checks the
/// files of each member with the includes and excludes of the config that governs it. A script is
/// passed by path. Pyrefly checks in the environment that `VIRTUAL_ENV` selects, which is the
/// script's environment for `uv check --script`.
pub(super) async fn run(
    pyrefly_path: &Path,
    target_dir: &Path,
    lock_check: LockCheck,
    frozen: Option<FrozenSource>,
    is_script: bool,
    check_targets: &[PathBuf],
    excluded_targets: &[PathBuf],
    venv: Option<&PythonEnvironment>,
    python_version: Option<&Version>,
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
    if let Some(python_version) = python_version {
        command
            .arg("--python-version")
            .arg(python_version.to_string());
    }
    if let Some(venv) = venv {
        command.env(EnvVars::VIRTUAL_ENV, venv.root());
    }
    // Pyrefly finds the environment of each PEP 723 script that it checks with this uv executable
    // and lockfile policy.
    if let Ok(uv) = std::env::current_exe() {
        for argument in [
            uv.as_os_str(),
            OsStr::new("workspace"),
            OsStr::new("metadata"),
            OsStr::new("--preview-features"),
            OsStr::new("workspace-metadata"),
            OsStr::new("--script"),
        ] {
            let mut flag = OsString::from("--script-command=");
            flag.push(argument);
            command.arg(flag);
        }
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

    // PEP 723 scripts have independent environments and must be checked explicitly with
    // `uv check --script`. This still allows explicitly selected script paths to be checked.
    command.arg("--exclude-scripts");
    for excluded_target in excluded_targets {
        command.arg("--project-excludes");
        command.arg(
            excluded_target
                .strip_prefix(target_dir)
                .unwrap_or(excluded_target),
        );
    }
    let relative_targets = check_targets.iter().map(|check_target| {
        let relative = check_target
            .strip_prefix(target_dir)
            .unwrap_or(check_target);
        // Pyrefly rejects an empty path, which is what the working directory strips to.
        if relative.as_os_str().is_empty() {
            Path::new(".")
        } else {
            relative
        }
    });
    if is_script {
        command.arg("--");
        command.args(relative_targets);
    } else {
        for relative_target in relative_targets {
            command.arg("--project-dir");
            command.arg(relative_target);
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
