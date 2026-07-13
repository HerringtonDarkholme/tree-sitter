use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Result};
use semver::{Version, VersionReq};

use crate::{root_dir, AstGrepGate};

const CORE_PACKAGES: &[&str] = &[
    "ast-grep-core",
    "ast-grep-config",
    "ast-grep-language",
    "ast-grep",
];

const FULL_PACKAGES: &[&str] = &[
    "ast-grep-core",
    "ast-grep-config",
    "ast-grep-language",
    "ast-grep",
    "ast-grep-outline",
    "ast-grep-dynamic",
    "ast-grep-lsp",
];

const BINDING_PACKAGES: &[&str] = &["ast-grep-napi", "ast-grep-py"];

pub fn run(args: &AstGrepGate) -> Result<()> {
    let root = root_dir();
    let ast_grep = args
        .ast_grep_path
        .clone()
        .unwrap_or_else(|| root.join("../../ast-grep"));
    ensure_dir(&ast_grep)?;

    let packages = if !args.packages.is_empty() {
        args.packages.iter().map(String::as_str).collect::<Vec<_>>()
    } else if args.full {
        FULL_PACKAGES.to_vec()
    } else {
        CORE_PACKAGES.to_vec()
    };
    let metadata = ast_grep_metadata(&ast_grep)?;
    let tree_sitter_version =
        resolve_tree_sitter_version(&metadata, args.tree_sitter_version.as_deref())?;
    preflight_ast_grep(&metadata, &packages, args.bindings, &tree_sitter_version)?;

    let compat_crate = prepare_tree_sitter_compat_crate(root, &tree_sitter_version)?;
    let ast_grep_workspace = prepare_ast_grep_workspace(&ast_grep)?;
    let cargo_patch_config = format!(
        "patch.crates-io.tree-sitter.path={}",
        toml_string(&compat_crate),
    );

    let target_dir = std::env::temp_dir().join("tree-sitter-ast-grep-gate-target");
    fs::create_dir_all(&target_dir)
        .with_context(|| format!("Failed to create {}", target_dir.display()))?;

    let mut command = Command::new("cargo");
    command
        .current_dir(&ast_grep_workspace)
        .env("CARGO_TARGET_DIR", &target_dir)
        .arg("test");

    for package in &packages {
        command.arg("-p").arg(package);
    }

    command
        .arg("--config")
        .arg(&cargo_patch_config)
        .arg("--lib")
        .arg("--tests");

    let output = command
        .output()
        .with_context(|| format!("Failed to run {command:?}"))?;

    if !output.status.success() {
        bail!(
            "ast-grep gate failed with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    println!(
        "ast-grep gate passed for {} packages using {}",
        packages.len(),
        compat_crate.display(),
    );

    if args.bindings {
        check_binding_packages(&ast_grep_workspace, &target_dir, &cargo_patch_config)?;
    }

    Ok(())
}

fn preflight_ast_grep(
    metadata: &AstGrepMetadata,
    packages: &[&str],
    bindings: bool,
    tree_sitter_version: &str,
) -> Result<()> {
    if metadata.tree_sitter_reqs.is_empty() {
        bail!("ast-grep workspace metadata does not contain a non-dev tree-sitter dependency");
    }
    let unexpected_reqs = metadata
        .tree_sitter_reqs
        .iter()
        .filter(|req| !dependency_req_matches_version(req, tree_sitter_version))
        .collect::<Vec<_>>();
    if !unexpected_reqs.is_empty() {
        bail!(
            "ast-grep workspace depends on tree-sitter requirement(s) {}, not {tree_sitter_version}. \
             Use --tree-sitter-version to match the ast-grep checkout.",
            unexpected_reqs
                .iter()
                .map(|req| req.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }

    for package in packages {
        if !metadata.package_names.contains(*package) {
            bail!(
                "Unknown ast-grep package '{package}'. Known packages: {}",
                metadata
                    .package_names
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
    }
    if bindings {
        for package in BINDING_PACKAGES {
            if !metadata.package_names.contains(*package) {
                bail!(
                    "Unknown ast-grep binding package '{package}'. Known packages: {}",
                    metadata
                        .package_names
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
        }
    }

    Ok(())
}

fn resolve_tree_sitter_version(
    metadata: &AstGrepMetadata,
    requested: Option<&str>,
) -> Result<String> {
    let version = if let Some(requested) = requested {
        requested.to_string()
    } else {
        metadata.locked_tree_sitter_version.clone().context(
            "ast-grep Cargo.lock does not contain tree-sitter; pass --tree-sitter-version",
        )?
    };

    Version::parse(&version).with_context(|| format!("Invalid tree-sitter version '{version}'"))?;
    Ok(version)
}

fn dependency_req_matches_version(req: &str, version: &str) -> bool {
    let Ok(version) = Version::parse(version) else {
        return false;
    };
    VersionReq::parse(req.trim()).is_ok_and(|req| req.matches(&version))
}

fn check_binding_packages(
    ast_grep: &Path,
    target_dir: &Path,
    cargo_patch_config: &str,
) -> Result<()> {
    run_cargo_check(
        ast_grep,
        target_dir,
        cargo_patch_config,
        "ast-grep NAPI binding gate",
        &["-p", "ast-grep-napi"],
    )?;
    run_cargo_check(
        ast_grep,
        target_dir,
        cargo_patch_config,
        "ast-grep Python binding gate",
        &["-p", "ast-grep-py", "--features", "python"],
    )?;

    println!(
        "ast-grep binding gate passed for {} packages",
        BINDING_PACKAGES.len(),
    );

    Ok(())
}

fn run_cargo_check(
    ast_grep: &Path,
    target_dir: &Path,
    cargo_patch_config: &str,
    label: &str,
    args: &[&str],
) -> Result<()> {
    let mut command = Command::new("cargo");
    command
        .current_dir(ast_grep)
        .env("CARGO_TARGET_DIR", target_dir)
        .arg("check");

    command.args(args).arg("--config").arg(cargo_patch_config);

    let output = command
        .output()
        .with_context(|| format!("Failed to run {command:?}"))?;

    if !output.status.success() {
        bail!(
            "{label} failed with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    Ok(())
}

fn ast_grep_metadata(ast_grep: &Path) -> Result<AstGrepMetadata> {
    let output = Command::new("cargo")
        .current_dir(ast_grep)
        .args(["metadata", "--no-deps", "--format-version", "1", "--quiet"])
        .output()
        .with_context(|| format!("Failed to run cargo metadata in {}", ast_grep.display()))?;

    if !output.status.success() {
        bail!(
            "cargo metadata failed in {}\nstdout:\n{}\nstderr:\n{}",
            ast_grep.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse ast-grep cargo metadata JSON")?;
    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .context("ast-grep cargo metadata did not contain a packages array")?;
    let mut package_names = BTreeSet::new();
    let mut tree_sitter_reqs = BTreeSet::new();

    for package in packages {
        if let Some(name) = package.get("name").and_then(serde_json::Value::as_str) {
            package_names.insert(name.to_string());
        }

        let Some(dependencies) = package
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for dependency in dependencies {
            let is_runtime_dependency = dependency
                .get("kind")
                .is_none_or(serde_json::Value::is_null);
            if is_runtime_dependency
                && dependency.get("name").and_then(serde_json::Value::as_str) == Some("tree-sitter")
            {
                let req = dependency
                    .get("req")
                    .and_then(serde_json::Value::as_str)
                    .context("tree-sitter dependency metadata is missing req")?;
                tree_sitter_reqs.insert(req.to_string());
            }
        }
    }

    Ok(AstGrepMetadata {
        package_names,
        tree_sitter_reqs,
        locked_tree_sitter_version: locked_tree_sitter_version(ast_grep)?,
    })
}

struct AstGrepMetadata {
    package_names: BTreeSet<String>,
    tree_sitter_reqs: BTreeSet<String>,
    locked_tree_sitter_version: Option<String>,
}

fn prepare_ast_grep_workspace(source: &Path) -> Result<PathBuf> {
    ensure_dir(source)?;

    let destination = std::env::temp_dir().join("tree-sitter-ast-grep-gate-ast-grep");
    if destination.exists() {
        fs::remove_dir_all(&destination)
            .with_context(|| format!("Failed to remove {}", destination.display()))?;
    }
    copy_dir_filtered(source, &destination)?;

    Ok(destination)
}

fn copy_dir_filtered(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("Failed to create {}", destination.display()))?;

    for entry in
        fs::read_dir(source).with_context(|| format!("Failed to read {}", source.display()))?
    {
        let entry = entry?;
        let file_name = entry.file_name();
        if should_skip_ast_grep_copy_entry(&file_name.to_string_lossy()) {
            continue;
        }

        let source_path = entry.path();
        let destination_path = destination.join(&file_name);
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            copy_dir_filtered(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "Failed to copy {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn should_skip_ast_grep_copy_entry(file_name: &str) -> bool {
    matches!(
        file_name,
        ".git"
            | ".env"
            | ".venv"
            | "node_modules"
            | "target"
            | "coverage"
            | "dist"
            | "build"
            | ".next"
            | ".turbo"
    )
}

fn locked_tree_sitter_version(ast_grep: &Path) -> Result<Option<String>> {
    let lockfile_path = ast_grep.join("Cargo.lock");
    if !lockfile_path.is_file() {
        return Ok(None);
    }

    let lockfile = fs::read_to_string(&lockfile_path)
        .with_context(|| format!("Failed to read {}", lockfile_path.display()))?;
    let mut in_package = false;
    let mut name = None;
    let mut version = None;

    for line in lockfile.lines().map(str::trim) {
        if line == "[[package]]" {
            if name.as_deref() == Some("tree-sitter") {
                return Ok(version);
            }
            in_package = true;
            name = None;
            version = None;
            continue;
        }

        if !in_package {
            continue;
        }

        if let Some(value) = lockfile_value(line, "name") {
            name = Some(value.to_string());
        } else if let Some(value) = lockfile_value(line, "version") {
            version = Some(value.to_string());
        }
    }

    if name.as_deref() == Some("tree-sitter") {
        return Ok(version);
    }

    Ok(None)
}

fn lockfile_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let value = line.strip_prefix(key)?.trim_start();
    let value = value.strip_prefix('=')?.trim_start();
    value.strip_prefix('"')?.split('"').next()
}

fn prepare_tree_sitter_compat_crate(root: &Path, version: &str) -> Result<PathBuf> {
    let source = root.join("lib");
    ensure_dir(&source)?;

    let destination = std::env::temp_dir()
        .join("tree-sitter-ast-grep-gate")
        .join(format!("tree-sitter-{version}"));
    if destination.exists() {
        fs::remove_dir_all(&destination)
            .with_context(|| format!("Failed to remove {}", destination.display()))?;
    }
    copy_dir(&source, &destination)?;
    fs::write(
        destination.join("Cargo.toml"),
        compatibility_manifest(version),
    )
    .with_context(|| {
        format!(
            "Failed to write {}",
            destination.join("Cargo.toml").display()
        )
    })?;

    Ok(destination)
}

fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("Failed to create {}", destination.display()))?;

    for entry in
        fs::read_dir(source).with_context(|| format!("Failed to read {}", source.display()))?
    {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            copy_dir(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "Failed to copy {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn ensure_dir(path: &Path) -> Result<()> {
    if path.is_dir() {
        Ok(())
    } else {
        bail!("Required directory does not exist: {}", path.display());
    }
}

fn toml_string(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}

fn compatibility_manifest(version: &str) -> String {
    format!(
        r#"[package]
name = "tree-sitter"
version = "{version}"
description = "Rust bindings to the Tree-sitter parsing library"
authors = [
  "Max Brunsfeld <maxbrunsfeld@gmail.com>",
  "Amaan Qureshi <amaanq12@gmail.com>",
]
edition = "2021"
rust-version = "1.77"
readme = "binding_rust/README.md"
homepage = "https://tree-sitter.github.io/tree-sitter"
repository = "https://github.com/tree-sitter/tree-sitter"
documentation = "https://docs.rs/tree-sitter"
license = "MIT"
keywords = ["incremental", "parsing"]
categories = [
  "api-bindings",
  "external-ffi-bindings",
  "parsing",
  "text-editors",
]
build = "binding_rust/build.rs"
links = "tree-sitter"

[features]
default = ["std"]
std = ["regex/std", "regex/perf", "regex-syntax/unicode"]

[dependencies]
regex = {{ version = "1.11.3", default-features = false, features = ["unicode"] }}
regex-syntax = {{ version = "0.8.6", default-features = false }}
tree-sitter-language = "0.1"
streaming-iterator = "0.1.9"

[build-dependencies]
bindgen = {{ version = "0.72.0", optional = true }}
cc = "1.2.54"
serde_json = {{ version = "1.0.149", features = ["preserve_order"] }}

[lib]
path = "binding_rust/lib.rs"
"#
    )
}
