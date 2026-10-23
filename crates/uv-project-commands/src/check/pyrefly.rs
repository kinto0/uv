use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::process::Command;

use uv_command_support::{ExitStatus, Printer, child::run_to_completion};
use uv_configuration::ColorChoice;
use uv_pep440::Version;
use uv_python_interpreter::PythonEnvironment;
use uv_shell::shlex_posix;

/// Run a type check powered by Pyrefly.
pub(super) async fn run(
    pyrefly_path: &Path,
    target_dir: &Path,
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
        // Pyrefly queries this interpreter for its site-packages and, unless overridden above, its
        // Python version.
        command
            .arg("--python-interpreter-path")
            .arg(venv.python_executable());
        command.env("VIRTUAL_ENV", venv.root());
    }
    for excluded_target in excluded_targets {
        command.arg("--project-excludes");
        command.arg(
            excluded_target
                .strip_prefix(target_dir)
                .unwrap_or(excluded_target),
        );
    }
    if !check_targets.is_empty() {
        command.arg("--");
        for check_target in check_targets {
            let relative = check_target
                .strip_prefix(target_dir)
                .unwrap_or(check_target);
            // Pyrefly rejects an empty path, which is what the working directory strips to.
            command.arg(if relative.as_os_str().is_empty() {
                Path::new(".")
            } else {
                relative
            });
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
