//! Describe the workspace as a Pyrefly source database.
//!
//! Pyrefly's `custom` build system runs a command as `<command> @<argfile>` and reads a
//! `TargetManifestDatabase` from its stdout. The argfile lists the files that Pyrefly checks, one
//! `--file <path>` pair at a time. This command answers that query from `uv workspace metadata`:
//!
//! - Each workspace member is a target whose sources are the member's own modules.
//! - Each environment is a target that holds every third-party module installed in it.
//! - Each requested PEP 723 script is a target that depends only on the script's own environment,
//!   so one Pyrefly run can check files from several environments.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use uv_command_support::{ExitStatus, Printer};
use uv_scripts::Pep723Script;

/// Print the Pyrefly source database for the files in `arguments`.
pub async fn pyrefly_source_db(arguments: Vec<String>, printer: Printer) -> Result<ExitStatus> {
    let files = requested_files(&arguments)?;
    let uv = std::env::current_exe().context("Failed to locate the uv executable")?;
    let platform = python_platform();

    let workspace = Metadata::query(&uv, None)?;
    let mut db = serde_json::Map::new();

    let environment_target = "//environment".to_owned();
    db.insert(
        environment_target.clone(),
        workspace.environment_target(platform),
    );

    for member in &workspace.members {
        let nested_members = workspace
            .members
            .iter()
            .filter(|other| other.path != member.path && other.path.starts_with(&member.path))
            .map(|other| other.path.as_path())
            .collect::<Vec<_>>();
        let deps = workspace
            .members
            .iter()
            .filter(|other| other.path != member.path)
            .map(|other| format!("//{}", other.name))
            .chain([environment_target.clone()])
            .collect::<Vec<_>>();
        db.insert(
            format!("//{}", member.name),
            json!({
                "srcs": member_sources(&member.path, &nested_members)?,
                "deps": deps,
                "buildfile_path": member.path.join("pyproject.toml"),
                "python_version": workspace.python_version(),
                "python_platform": platform,
            }),
        );
    }

    for file in files {
        if Pep723Script::read(&file).await?.is_none() {
            continue;
        }
        let script = Metadata::query(&uv, Some(&file))?;
        let script_environment_target = format!("//script-environment:{}", file.display());
        db.insert(
            script_environment_target.clone(),
            script.environment_target(platform),
        );
        let Some(module) = file.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        db.insert(
            format!("//script:{}", file.display()),
            json!({
                "srcs": { module: [&file] },
                "deps": [script_environment_target],
                "python_version": script.python_version(),
                "python_platform": platform,
            }),
        );
    }

    let manifest = json!({ "db": db, "root": workspace.workspace_root });
    writeln!(printer.stdout(), "{manifest}")?;
    Ok(ExitStatus::Success)
}

/// Read the file paths from Pyrefly's `@<argfile>` arguments.
fn requested_files(arguments: &[String]) -> Result<Vec<PathBuf>> {
    let mut words = Vec::new();
    for argument in arguments {
        if let Some(argfile) = argument.strip_prefix('@') {
            let contents = fs_err::read_to_string(argfile)?;
            words.extend(contents.lines().map(str::to_owned));
        } else {
            words.push(argument.clone());
        }
    }
    let mut files = Vec::new();
    let mut words = words.into_iter();
    while let Some(word) = words.next() {
        if word == "--file"
            && let Some(file) = words.next()
        {
            files.push(PathBuf::from(file));
        }
    }
    Ok(files)
}

fn python_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// The subset of `uv workspace metadata` output that a source database needs.
#[derive(Deserialize)]
struct Metadata {
    workspace_root: Option<PathBuf>,
    environment: Environment,
    #[serde(default)]
    module_owners: BTreeMap<String, Vec<ModuleOwner>>,
    #[serde(default)]
    members: Vec<Member>,
}

#[derive(Deserialize)]
struct Environment {
    python: EnvironmentPython,
    #[serde(default)]
    packages: BTreeMap<String, InstalledPackage>,
}

#[derive(Deserialize)]
struct EnvironmentPython {
    version: String,
}

#[derive(Deserialize)]
struct InstalledPackage {
    path: PathBuf,
}

#[derive(Deserialize)]
struct ModuleOwner {
    package_id: String,
}

#[derive(Deserialize)]
struct Member {
    name: String,
    path: PathBuf,
}

impl Metadata {
    /// Run `uv workspace metadata` for the workspace, or for a script and its environment.
    ///
    /// The child inherits `UV_FROZEN`, `UV_LOCKED`, and the other settings that `uv check` sets.
    fn query(uv: &Path, script: Option<&Path>) -> Result<Self> {
        let mut command = Command::new(uv);
        command.args([
            "workspace",
            "metadata",
            "--preview-features",
            "workspace-metadata",
        ]);
        if let Some(script) = script {
            command.arg("--sync").arg("--script").arg(script);
        }
        let output = command
            .output()
            .context("Failed to run `uv workspace metadata`")?;
        if !output.status.success() {
            anyhow::bail!(
                "`uv workspace metadata` failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        serde_json::from_slice(&output.stdout)
            .context("Failed to parse `uv workspace metadata` output")
    }

    /// The `major.minor` version of the environment's interpreter.
    fn python_version(&self) -> String {
        self.environment
            .python
            .version
            .split('.')
            .take(2)
            .collect::<Vec<_>>()
            .join(".")
    }

    /// A target that holds every module that an installed distribution owns.
    fn environment_target(&self, platform: &str) -> Value {
        let mut sources = BTreeMap::<&str, Vec<PathBuf>>::new();
        for (module, owners) in &self.module_owners {
            for owner in owners {
                if let Some(package) = self.environment.packages.get(&owner.package_id)
                    && let Some(site_packages) = package.path.parent()
                    && let Some(path) = module_file(site_packages, module)
                {
                    sources.entry(module).or_default().push(path);
                }
            }
        }
        json!({
            "srcs": sources,
            "deps": [],
            "python_version": self.python_version(),
            "python_platform": platform,
        })
    }
}

/// Find the stub or source file for `module` under `root`.
fn module_file(root: &Path, module: &str) -> Option<PathBuf> {
    let base = module
        .split('.')
        .fold(root.to_path_buf(), |path, part| path.join(part));
    [
        base.with_extension("pyi"),
        base.with_extension("py"),
        base.join("__init__.pyi"),
        base.join("__init__.py"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

/// Map each module in a workspace member to its files.
///
/// The member's import root is its `src` directory when one exists, and the member root otherwise.
/// Nested workspace members and hidden directories are skipped.
fn member_sources(root: &Path, nested_members: &[&Path]) -> Result<BTreeMap<String, Vec<PathBuf>>> {
    let src = root.join("src");
    let import_root = if src.is_dir() {
        src
    } else {
        root.to_path_buf()
    };
    let mut sources = BTreeMap::<String, Vec<PathBuf>>::new();
    let walker = walkdir::WalkDir::new(&import_root)
        .into_iter()
        .filter_entry(|entry| {
            let hidden = entry.depth() > 0
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with('.'));
            !hidden && !nested_members.contains(&entry.path())
        });
    for entry in walker {
        let entry = entry?;
        let path = entry.path();
        let is_python = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "py" || extension == "pyi");
        if !entry.file_type().is_file() || !is_python {
            continue;
        }
        let Ok(relative) = path
            .with_extension("")
            .strip_prefix(&import_root)
            .map(Path::to_path_buf)
        else {
            continue;
        };
        let mut parts = relative
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .collect::<Vec<_>>();
        if parts.last() == Some(&"__init__") {
            parts.pop();
        }
        if parts.is_empty() {
            continue;
        }
        sources
            .entry(parts.join("."))
            .or_default()
            .push(path.to_path_buf());
    }
    Ok(sources)
}
