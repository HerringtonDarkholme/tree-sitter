mod ast_grep_gate;
mod benchmark;
mod build_wasm;
mod bump;
mod check_wasm_exports;
mod clippy;
mod core_parity;
mod embed_sources;
mod fetch;
mod generate;
mod migration_gate;
mod perf_gate;
mod test;
mod test_schema;

use std::{path::Path, process::Command};

use anstyle::{AnsiColor, Color, Style};
use anyhow::Result;
use clap::{crate_authors, Args, FromArgMatches as _, Subcommand};
use semver::Version;

#[derive(Subcommand)]
#[command(about="Run various tasks", author=crate_authors!("\n"), styles=get_styles())]
enum Commands {
    /// Run ast-grep tests against this local Tree-sitter crate.
    AstGrepGate(AstGrepGate),
    /// Runs `cargo benchmark` with some optional environment variables set.
    Benchmark(Benchmark),
    /// Compile the Tree-sitter Wasm library. This will create two files in the
    /// `lib/binding_web` directory: `web-tree-sitter.js` and `web-tree-sitter.wasm`.
    BuildWasm(BuildWasm),
    /// Bumps the version of the workspace.
    BumpVersion(BumpVersion),
    /// Checks that Wasm exports are synced.
    CheckWasmExports(CheckWasmExports),
    /// Runs `cargo clippy`.
    Clippy(Clippy),
    /// Compare Rust core behavior against the original C core.
    CoreParity(CoreParity),
    /// Fetches emscripten.
    FetchEmscripten,
    /// Fetches the fixtures for testing tree-sitter.
    FetchFixtures,
    /// Generate the Rust bindings from the C library.
    GenerateBindings,
    /// Generates the fixtures for testing tree-sitter.
    GenerateFixtures(GenerateFixtures),
    /// Generates the JSON schema for the test runner summary.
    GenerateTestSchema,
    /// Generate the list of exports from Tree-sitter Wasm files.
    GenerateWasmExports,
    /// Run the broader Rust-core migration gate.
    MigrationGate(MigrationGate),
    /// Compare Rust-core benchmark performance against the old C core.
    PerfGate(PerfGate),
    /// Run the test suite
    Test(Test),
    /// Run the Wasm test suite
    TestWasm,
}

#[derive(Args)]
struct Benchmark {
    /// The language to run the benchmarks for.
    #[arg(long, short)]
    language: Option<String>,
    /// The example file to run the benchmarks for.
    #[arg(long, short)]
    example_file_name: Option<String>,
    /// The number of times to parse each sample (default is 5).
    #[arg(long, short, default_value = "5")]
    repetition_count: u32,
    /// Benchmark case kind to run: query, normal, error, or all.
    #[arg(long, default_value = "all")]
    kind: String,
    /// Whether to run the benchmarks in debug mode.
    #[arg(long, short = 'g')]
    debug: bool,
}

#[derive(Args)]
struct AstGrepGate {
    /// ast-grep repository path. Defaults to ../../ast-grep from this repo.
    #[arg(long)]
    ast_grep_path: Option<std::path::PathBuf>,
    /// ast-grep package(s) to test. Defaults to core/config/language/CLI packages.
    #[arg(long, short = 'p')]
    packages: Vec<String>,
    /// Version to expose from the temporary tree-sitter compatibility crate.
    /// Defaults to ast-grep's locked tree-sitter version.
    #[arg(long)]
    tree_sitter_version: Option<String>,
    /// Include outline, dynamic, and lsp packages in addition to the core gate.
    #[arg(long)]
    full: bool,
    /// Also compile-check ast-grep's NAPI and Python binding crates.
    #[arg(long)]
    bindings: bool,
}

#[derive(Args)]
struct BuildWasm {
    /// Compile the library more quickly, with fewer optimizations
    /// and more runtime assertions.
    #[arg(long, short = '0')]
    debug: bool,
    /// Run emscripten using docker, even if \`emcc\` is installed.
    /// By default, \`emcc\` will be run directly when available.
    #[arg(long, short)]
    docker: bool,
    /// Run emscripten with verbose output.
    #[arg(long, short)]
    verbose: bool,
    /// Rebuild when relevant files are changed.
    #[arg(long, short)]
    watch: bool,
    /// Emit TypeScript type definitions for the generated bindings,
    /// requires `tsc` to be available.
    #[arg(long, short)]
    emit_tsd: bool,
    /// Generate `CommonJS` modules instead of ES modules.
    #[arg(long, short, env = "CJS")]
    cjs: bool,
}

#[derive(Args)]
struct BumpVersion {
    /// The version to bump to.
    #[arg(index = 1, required = true)]
    version: Version,
}

#[derive(Args)]
struct CheckWasmExports {
    /// Recheck when relevant files are changed.
    #[arg(long, short)]
    watch: bool,
}

#[derive(Args)]
struct Clippy {
    /// Automatically apply lint suggestions (`clippy --fix`).
    #[arg(long, short)]
    fix: bool,
    /// The package to run Clippy against (`cargo -p <PACKAGE> clippy`).
    #[arg(long, short)]
    package: Option<String>,
}

#[derive(Args)]
struct CoreParity {
    /// tree-sitter-typescript repository path. Defaults to ../tree-sitter-typescript.
    #[arg(long)]
    tree_sitter_typescript_path: Option<std::path::PathBuf>,
    /// TypeScript repository path. Defaults to ../typescript.
    #[arg(long)]
    typescript_path: Option<std::path::PathBuf>,
    /// Maximum number of TypeScript source samples to parse.
    #[arg(long, default_value = "6")]
    sample_limit: usize,
    /// Maximum number of tree-sitter-typescript corpus examples to parse.
    #[arg(long, default_value = "8")]
    corpus_sample_limit: usize,
    /// Git revision whose lib/src directory contains the original C core.
    #[arg(long, default_value = "c9f80282ad355a88a389d75173d918de84ef3e79")]
    c_core_rev: String,
}

#[derive(Args)]
struct GenerateFixtures {
    /// Generates the parser to Wasm
    #[arg(long, short)]
    wasm: bool,
}

#[derive(Args)]
struct MigrationGate {
    /// ast-grep repository path. Defaults to ../../ast-grep from this repo.
    #[arg(long)]
    ast_grep_path: Option<std::path::PathBuf>,
    /// tree-sitter-typescript repository path. Defaults to ../tree-sitter-typescript.
    #[arg(long)]
    tree_sitter_typescript_path: Option<std::path::PathBuf>,
    /// TypeScript repository path. Defaults to ../typescript.
    #[arg(long)]
    typescript_path: Option<std::path::PathBuf>,
    /// Maximum number of TypeScript source samples to parse.
    #[arg(long, default_value = "6")]
    sample_limit: usize,
    /// Maximum number of tree-sitter-typescript corpus examples to parse.
    #[arg(long, default_value = "8")]
    corpus_sample_limit: usize,
    /// Git revision whose lib/src directory contains the original C core.
    #[arg(long, default_value = "c9f80282ad355a88a389d75173d918de84ef3e79")]
    c_core_rev: String,
    /// Version to expose from the temporary tree-sitter compatibility crate.
    /// Defaults to ast-grep's locked tree-sitter version.
    #[arg(long)]
    tree_sitter_version: Option<String>,
    /// Run `cargo xtask fetch-fixtures` before workspace tests.
    #[arg(long)]
    fetch_fixtures: bool,
    /// Include outline, dynamic, and LSP ast-grep packages.
    #[arg(long)]
    ast_grep_full: bool,
    /// Compile-check ast-grep's NAPI and Python binding crates.
    #[arg(long)]
    ast_grep_bindings: bool,
    /// Pass `--offline` to Cargo test commands.
    #[arg(long)]
    offline: bool,
    /// Skip old-C-core vs Rust-core differential checks.
    #[arg(long)]
    skip_core_parity: bool,
    /// Skip the tree-sitter workspace Rust test suite.
    #[arg(long)]
    skip_workspace_tests: bool,
    /// Skip ast-grep consumer tests.
    #[arg(long)]
    skip_ast_grep: bool,
    /// Also build the `SwiftPM` package.
    #[arg(long)]
    swift_build: bool,
}

#[derive(Args)]
struct PerfGate {
    /// Language(s) to benchmark. Defaults to a representative core parser set.
    #[arg(long = "language", alias = "languages", short = 'l')]
    languages: Vec<String>,
    /// The number of times to parse each sample.
    #[arg(long, default_value = "10")]
    repetitions: usize,
    /// Benchmark case kind to compare: normal, error, or all.
    #[arg(long, default_value = "normal")]
    kind: String,
    /// Maximum number of mismatched-language error samples per other language.
    #[arg(long, default_value = "8")]
    error_limit: usize,
    /// TypeScript repository path. Defaults to ../typescript when present.
    #[arg(long)]
    typescript_path: Option<std::path::PathBuf>,
    /// Git revision whose lib/src directory contains the original C core.
    #[arg(long, default_value = "c9f80282ad355a88a389d75173d918de84ef3e79")]
    c_core_rev: String,
    /// Maximum allowed per-case Rust slowdown versus C before strict mode fails.
    #[arg(long, default_value = "5.0")]
    max_regression_percent: f64,
    /// Required weighted overall Rust speedup over C before strict mode passes.
    #[arg(long, default_value = "0.0")]
    min_overall_speedup_percent: f64,
    /// Print results without failing on regressions.
    #[arg(long)]
    report_only: bool,
    /// Pass `--offline` to Cargo benchmark commands.
    #[arg(long)]
    offline: bool,
}

#[derive(Args)]
struct Test {
    /// Compile C code with the Clang address sanitizer.
    #[arg(long, short)]
    address_sanitizer: bool,
    /// Run only the corpus tests for the given language.
    #[arg(long, short)]
    language: Option<String>,
    /// Run only the corpus tests whose name contain the given string.
    #[arg(long, short)]
    example: Option<String>,
    /// Run the given number of iterations of randomized tests (default 10).
    #[arg(long, short)]
    iterations: Option<u32>,
    /// Set the seed used to control random behavior.
    #[arg(long, short)]
    seed: Option<usize>,
    /// Print parsing log to stderr.
    #[arg(long, short)]
    debug: bool,
    /// Generate an SVG graph of parsing logs.
    #[arg(long, short = 'D')]
    debug_graph: bool,
    /// Run the tests with a debugger.
    #[arg(short)]
    g: bool,
    #[arg(trailing_var_arg = true)]
    args: Vec<String>,
    /// Don't capture the output
    #[arg(long)]
    nocapture: bool,
}

const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILD_SHA: Option<&str> = option_env!("BUILD_SHA");
const EMSCRIPTEN_VERSION: &str = include_str!("../../loader/emscripten-version").trim_ascii();
const EMSCRIPTEN_TAG: &str = concat!(
    "docker.io/emscripten/emsdk:",
    include_str!("../../loader/emscripten-version")
)
.trim_ascii();

fn main() {
    let result = run();
    if let Err(err) = &result {
        // Ignore BrokenPipe errors
        if let Some(error) = err.downcast_ref::<std::io::Error>() {
            if error.kind() == std::io::ErrorKind::BrokenPipe {
                return;
            }
        }
        if !err.to_string().is_empty() {
            eprintln!("{err:?}");
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let version = BUILD_SHA.map_or_else(
        || BUILD_VERSION.to_string(),
        |build_sha| format!("{BUILD_VERSION} ({build_sha})"),
    );
    let version: &'static str = Box::leak(version.into_boxed_str());

    let cli = clap::Command::new("xtask")
        .help_template(
            "\
{before-help}{name} {version}
{author-with-newline}{about-with-newline}
{usage-heading} {usage}

{all-args}{after-help}
",
        )
        .version(version)
        .subcommand_required(true)
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .disable_colored_help(false);
    let command = Commands::from_arg_matches(&Commands::augment_subcommands(cli).get_matches())?;

    match command {
        Commands::AstGrepGate(ast_grep_gate_options) => ast_grep_gate::run(&ast_grep_gate_options)?,
        Commands::Benchmark(benchmark_options) => benchmark::run(&benchmark_options)?,
        Commands::BuildWasm(build_wasm_options) => build_wasm::run_wasm(&build_wasm_options)?,
        Commands::BumpVersion(bump_options) => bump::run(bump_options)?,
        Commands::CheckWasmExports(check_options) => check_wasm_exports::run(&check_options)?,
        Commands::Clippy(clippy_options) => clippy::run(&clippy_options)?,
        Commands::CoreParity(core_parity_options) => core_parity::run(&core_parity_options)?,
        Commands::FetchEmscripten => fetch::run_emscripten()?,
        Commands::FetchFixtures => {
            fetch::run_fixtures()?;
        }
        Commands::GenerateBindings => generate::run_bindings()?,
        Commands::GenerateFixtures(generate_fixtures_options) => {
            generate::run_fixtures(&generate_fixtures_options)?;
        }
        Commands::GenerateTestSchema => test_schema::run_test_schema()?,
        Commands::GenerateWasmExports => generate::run_wasm_exports()?,
        Commands::MigrationGate(migration_gate_options) => {
            migration_gate::run(&migration_gate_options)?;
        }
        Commands::PerfGate(perf_gate_options) => perf_gate::run(&perf_gate_options)?,
        Commands::Test(test_options) => test::run(&test_options)?,
        Commands::TestWasm => test::run_wasm()?,
    }

    Ok(())
}

fn root_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
}

fn bail_on_err(output: &std::process::Output, prefix: &str) -> Result<()> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{prefix}:\n{stderr}");
    }
    Ok(())
}

#[must_use]
const fn get_styles() -> clap::builder::Styles {
    clap::builder::Styles::styled()
        .usage(
            Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Yellow))),
        )
        .header(
            Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Yellow))),
        )
        .literal(Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green))))
        .invalid(
            Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Red))),
        )
        .error(
            Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Red))),
        )
        .valid(
            Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Green))),
        )
        .placeholder(Style::new().fg_color(Some(Color::Ansi(AnsiColor::White))))
}

pub fn create_commit(msg: &str, paths: &[&str]) -> Result<String> {
    for path in paths {
        let output = Command::new("git").args(["add", path]).output()?;
        if !output.status.success() {
            anyhow::bail!(
                "Failed to add {path}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    let output = Command::new("git").args(["commit", "-m", msg]).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "Failed to commit: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = Command::new("git").args(["rev-parse", "HEAD"]).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "Failed to get commit SHA: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

#[macro_export]
macro_rules! watch_wasm {
    ($watch_fn:expr) => {
        if let Err(e) = $watch_fn() {
            eprintln!("{e}");
        } else {
            println!("Build succeeded");
        }

        let watch_files = [
            "lib/tree-sitter.c",
            "lib/exports.txt",
            "lib/imports.js",
            "lib/prefix.js",
        ]
        .iter()
        .map(PathBuf::from)
        .collect::<HashSet<PathBuf>>();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut debouncer = new_debouncer(Duration::from_secs(1), None, tx)?;
        debouncer.watch("lib/binding_web", RecursiveMode::NonRecursive)?;

        for result in rx {
            match result {
                Ok(events) => {
                    for event in events {
                        if event.kind == EventKind::Access(AccessKind::Close(AccessMode::Write))
                            && event
                                .paths
                                .iter()
                                .filter_map(|p| p.file_name())
                                .any(|p| watch_files.contains(&PathBuf::from(p)))
                        {
                            if let Err(e) = $watch_fn() {
                                eprintln!("{e}");
                            } else {
                                println!("Build succeeded");
                            }
                        }
                    }
                }
                Err(errors) => {
                    return Err(anyhow!(
                        "{}",
                        errors
                            .into_iter()
                            .map(|e| e.to_string())
                            .collect::<Vec<_>>()
                            .join("\n")
                    ));
                }
            }
        }
    };
}
