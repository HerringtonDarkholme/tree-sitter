use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use anyhow::{bail, Context, Result};

use crate::{ast_grep_gate, core_parity, fetch, root_dir, AstGrepGate, CoreParity, MigrationGate};

pub fn run(args: &MigrationGate) -> Result<()> {
    if !args.skip_core_parity {
        print_step("core parity");
        core_parity::run(&CoreParity {
            tree_sitter_typescript_path: args.tree_sitter_typescript_path.clone(),
            typescript_path: args.typescript_path.clone(),
            sample_limit: args.sample_limit,
            corpus_sample_limit: args.corpus_sample_limit,
            c_core_rev: args.c_core_rev.clone(),
        })?;
    }

    if args.fetch_fixtures {
        print_step("fixture grammars");
        fetch::run_fixtures()?;
    }

    if !args.skip_workspace_tests {
        print_step("workspace tests");
        ensure_fixture_grammars(root_dir())?;
        run_workspace_tests(args.offline)?;
    }

    if !args.skip_ast_grep {
        print_step("ast-grep consumer gate");
        ast_grep_gate::run(&AstGrepGate {
            ast_grep_path: args.ast_grep_path.clone(),
            packages: Vec::new(),
            tree_sitter_version: args.tree_sitter_version.clone(),
            full: args.ast_grep_full,
            bindings: args.ast_grep_bindings,
        })?;
    }

    if args.swift_build {
        print_step("SwiftPM package build");
        run_swift_build()?;
    }

    println!("migration gate passed");
    Ok(())
}

fn print_step(name: &str) {
    println!("\n==> {name}");
}

fn ensure_fixture_grammars(root: &Path) -> Result<()> {
    let fixtures_path = root.join("test/fixtures/fixtures.json");
    let grammars_dir = root.join("test/fixtures/grammars");
    let fixtures: Vec<(String, String, Option<String>)> =
        serde_json::from_str(&fs::read_to_string(&fixtures_path)?)?;

    let missing = fixtures
        .into_iter()
        .map(|(grammar, _, _)| grammar)
        .filter_map(
            |grammar| match contains_grammar_json(&grammars_dir.join(&grammar)) {
                Ok(true) => None,
                Ok(false) => Some(Ok(grammar)),
                Err(err) => Some(Err(err)),
            },
        )
        .collect::<Result<Vec<_>>>()?;

    if !missing.is_empty() {
        bail!(
            "missing fixture grammars: {}\nrun `cargo xtask fetch-fixtures`, or rerun this command with `--fetch-fixtures`",
            missing.join(", ")
        );
    }

    Ok(())
}

fn contains_grammar_json(path: &Path) -> Result<bool> {
    if path.join("src/grammar.json").is_file() {
        return Ok(true);
    }

    if !path.is_dir() {
        return Ok(false);
    }

    for entry in fs::read_dir(path).with_context(|| format!("Failed to read {}", path.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && contains_grammar_json(&entry.path())? {
            return Ok(true);
        }
    }

    Ok(false)
}

fn run_workspace_tests(offline: bool) -> Result<()> {
    let root = root_dir();
    let cache_dir = std::env::temp_dir().join("tree-sitter-migration-gate-cache");
    let lib_dir = std::env::temp_dir().join("tree-sitter-migration-gate-libdir");
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("Failed to create {}", cache_dir.display()))?;
    fs::create_dir_all(&lib_dir)
        .with_context(|| format!("Failed to create {}", lib_dir.display()))?;

    let mut command = Command::new("cargo");
    command
        .current_dir(root)
        .env("XDG_CACHE_HOME", &cache_dir)
        .env("TREE_SITTER_LIBDIR", &lib_dir)
        .arg("test")
        .arg("--workspace")
        .arg("--lib")
        .arg("--tests");
    if offline {
        command.arg("--offline");
    }

    run_command(command, "workspace tests")
}

fn run_swift_build() -> Result<()> {
    let root = root_dir();
    let mut command = Command::new("swift");
    command
        .current_dir(root)
        .env(
            "CLANG_MODULE_CACHE_PATH",
            std::env::temp_dir().join("tree-sitter-swift-clang-cache"),
        )
        .env(
            "SWIFTPM_MODULECACHE_PATH",
            std::env::temp_dir().join("tree-sitter-swift-module-cache"),
        )
        .arg("build")
        .arg("--scratch-path")
        .arg(std::env::temp_dir().join("tree-sitter-swift-build"));

    run_command(command, "SwiftPM package build")
}

fn run_command(mut command: Command, label: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("Failed to run {command:?}"))?;
    if !output.status.success() {
        bail!("{label} failed\n{}", describe_output(&output));
    }
    Ok(())
}

fn describe_output(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}
