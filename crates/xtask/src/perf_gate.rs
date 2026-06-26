use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::{core_parity, root_dir, PerfGate};

const DEFAULT_LANGUAGES: &[&str] = &[
    "cpp",
    "go",
    "java",
    "javascript",
    "python",
    "rust",
    "typescript",
];

pub fn run(args: &PerfGate) -> Result<()> {
    let root = root_dir();
    core_parity::preflight_c_core_revision(root, &args.c_core_rev)?;
    let c_core_src_dir = core_parity::materialize_c_core(root, &args.c_core_rev)?;

    let languages = languages(args);
    let mut all_comparisons = Vec::new();

    for language in &languages {
        println!("\n==> {language}: Rust core");
        let rust_results = run_benchmark(root, args, language, CoreImpl::Rust, None)?;

        println!("==> {language}: C core");
        let c_results = run_benchmark(root, args, language, CoreImpl::C, Some(&c_core_src_dir))?;

        let comparisons = compare_measurements(args, language, &rust_results, &c_results)?;
        print_language_summary(args, language, &comparisons)?;
        all_comparisons.extend(comparisons);
    }

    if all_comparisons.is_empty() {
        bail!("no shared parser benchmark cases were measured");
    }

    print_overall_summary(args, &all_comparisons);
    enforce_thresholds(args, &all_comparisons)?;

    Ok(())
}

fn languages(args: &PerfGate) -> Vec<String> {
    if args.languages.is_empty() {
        DEFAULT_LANGUAGES
            .iter()
            .map(|language| (*language).into())
            .collect()
    } else {
        args.languages.clone()
    }
}

fn run_benchmark(
    root: &Path,
    args: &PerfGate,
    language: &str,
    core_impl: CoreImpl,
    c_core_src_dir: Option<&Path>,
) -> Result<BTreeMap<CaseKey, Measurement>> {
    let mut command = Command::new("cargo");
    command
        .current_dir(root)
        .env("TREE_SITTER_CORE_IMPL", core_impl.env_value())
        .env("TREE_SITTER_BENCHMARK_LANGUAGE_FILTER", language)
        .env(
            "TREE_SITTER_BENCHMARK_REPETITION_COUNT",
            args.repetitions.to_string(),
        )
        .env("TREE_SITTER_BENCHMARK_KIND_FILTER", benchmark_kind(args)?)
        .env(
            "TREE_SITTER_BENCHMARK_ERROR_LIMIT",
            args.error_limit.to_string(),
        )
        .env(
            "CARGO_TARGET_DIR",
            target_dir_for_core(core_impl, &args.c_core_rev),
        )
        .arg("bench")
        .arg("benchmark")
        .arg("-p")
        .arg("tree-sitter-cli");

    if let Some(path) = &args.typescript_path {
        command.env("TREE_SITTER_BENCHMARK_TYPESCRIPT_PATH", path);
    }

    if let Some(src_dir) = c_core_src_dir {
        command.env("TREE_SITTER_C_CORE_SRC_DIR", src_dir);
    }

    if args.offline {
        command.arg("--offline");
    }

    let output = command
        .output()
        .with_context(|| format!("Failed to run {command:?}"))?;
    if !output.status.success() {
        bail!(
            "{core_impl:?} benchmark for {language} failed\n{}",
            describe_output(&output)
        );
    }

    let measurements = parse_benchmark_results(&output.stdout).with_context(|| {
        format!("Failed to parse {core_impl:?} benchmark output for {language}")
    })?;
    if measurements.is_empty() {
        bail!("{core_impl:?} benchmark for {language} produced no BENCHMARK_RESULT lines");
    }

    Ok(measurements)
}

fn target_dir_for_core(core_impl: CoreImpl, rev: &str) -> PathBuf {
    std::env::temp_dir()
        .join("tree-sitter-perf-gate-target")
        .join(core_impl.env_value())
        .join(sanitize_path_component(rev))
}

fn parse_benchmark_results(output: &[u8]) -> Result<BTreeMap<CaseKey, Measurement>> {
    let output = String::from_utf8_lossy(output);
    let mut measurements = BTreeMap::new();
    for line in output.lines() {
        let Some(json) = line.strip_prefix("BENCHMARK_RESULT ") else {
            continue;
        };
        let value: Value = serde_json::from_str(json)?;
        let key = CaseKey {
            language: required_str(&value, "language")?.into(),
            kind: required_str(&value, "kind")?.into(),
            path: required_str(&value, "path")?.into(),
        };
        let measurement = Measurement {
            bytes: required_u64(&value, "bytes")?,
            duration_ns: required_u64(&value, "duration_ns")?.max(1),
        };
        measurements.insert(key, measurement);
    }
    Ok(measurements)
}

fn compare_measurements(
    args: &PerfGate,
    language: &str,
    rust_results: &BTreeMap<CaseKey, Measurement>,
    c_results: &BTreeMap<CaseKey, Measurement>,
) -> Result<Vec<Comparison>> {
    let rust_keys = rust_results.keys().collect::<BTreeSet<_>>();
    let c_keys = c_results.keys().collect::<BTreeSet<_>>();
    let missing_rust = c_keys.difference(&rust_keys).count();
    let missing_c = rust_keys.difference(&c_keys).count();
    if missing_rust != 0 || missing_c != 0 {
        println!(
            "  shared-case note: {missing_rust} C cases missing from Rust output, {missing_c} Rust cases missing from C output"
        );
    }

    let parser_kinds = parser_kinds(args)?;
    let comparisons = rust_results
        .iter()
        .filter_map(|(key, rust)| {
            if !is_parser_case(&parser_kinds, key) {
                return None;
            }
            c_results.get(key).map(|c| Comparison {
                key: key.clone(),
                rust: *rust,
                c: *c,
            })
        })
        .collect::<Vec<_>>();

    if comparisons.is_empty() {
        bail!("no shared parser benchmark cases for language {language}");
    }

    Ok(comparisons)
}

fn print_language_summary(
    args: &PerfGate,
    language: &str,
    comparisons: &[Comparison],
) -> Result<()> {
    println!("  {language} parser throughput:");
    for kind in parser_kinds(args)? {
        let aggregate = aggregate(
            comparisons
                .iter()
                .filter(|comparison| comparison.key.kind == kind.as_str()),
        );
        let Some(aggregate) = aggregate else {
            continue;
        };
        println!(
            "    {kind:<6} cases {:>3}  Rust {:>10.1} bytes/ms  C {:>10.1} bytes/ms  delta {:+6.2}%",
            aggregate.cases,
            aggregate.rust_speed(),
            aggregate.c_speed(),
            aggregate.rust_delta_percent(),
        );
    }
    Ok(())
}

fn print_overall_summary(args: &PerfGate, comparisons: &[Comparison]) {
    let aggregate = aggregate(comparisons.iter()).expect("comparisons are not empty");
    println!("\n==> Overall parser throughput");
    println!(
        "    cases {:>3}  Rust {:>10.1} bytes/ms  C {:>10.1} bytes/ms  delta {:+6.2}%",
        aggregate.cases,
        aggregate.rust_speed(),
        aggregate.c_speed(),
        aggregate.rust_delta_percent(),
    );

    let regressions = regressions(args, comparisons);
    if regressions.is_empty() {
        println!(
            "    no per-case regressions above {:.2}%",
            args.max_regression_percent
        );
    } else {
        println!(
            "    per-case regressions above {:.2}%:",
            args.max_regression_percent
        );
        for regression in regressions.iter().take(10) {
            println!(
                "    - {} {} {}: Rust {:.1} bytes/ms, C {:.1} bytes/ms, slowdown {:.2}%",
                regression.key.language,
                regression.key.kind,
                display_case_path(&regression.key.path),
                regression.rust.speed(),
                regression.c.speed(),
                regression.rust_slowdown_percent(),
            );
        }
    }
}

fn enforce_thresholds(args: &PerfGate, comparisons: &[Comparison]) -> Result<()> {
    if args.report_only {
        println!("perf gate report-only mode: not enforcing thresholds");
        return Ok(());
    }

    let aggregate = aggregate(comparisons.iter()).expect("comparisons are not empty");
    let overall_delta = aggregate.rust_delta_percent();
    let regressions = regressions(args, comparisons);

    if overall_delta < args.min_overall_speedup_percent || !regressions.is_empty() {
        bail!(
            "perf gate failed: overall Rust delta {overall_delta:+.2}% (required {:+.2}%), {} per-case regressions above {:.2}%",
            args.min_overall_speedup_percent,
            regressions.len(),
            args.max_regression_percent
        );
    }

    println!("perf gate passed");
    Ok(())
}

fn regressions<'a>(args: &PerfGate, comparisons: &'a [Comparison]) -> Vec<&'a Comparison> {
    let mut regressions = comparisons
        .iter()
        .filter(|comparison| comparison.rust_slowdown_percent() > args.max_regression_percent)
        .collect::<Vec<_>>();
    regressions.sort_by(|a, b| {
        b.rust_slowdown_percent()
            .partial_cmp(&a.rust_slowdown_percent())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    regressions
}

fn aggregate<'a>(comparisons: impl Iterator<Item = &'a Comparison>) -> Option<Aggregate> {
    let mut aggregate = Aggregate::default();
    for comparison in comparisons {
        aggregate.cases += 1;
        aggregate.rust_bytes += comparison.rust.bytes;
        aggregate.rust_duration_ns += comparison.rust.duration_ns;
        aggregate.c_bytes += comparison.c.bytes;
        aggregate.c_duration_ns += comparison.c.duration_ns;
    }
    (aggregate.cases != 0).then_some(aggregate)
}

fn is_parser_case(parser_kinds: &[String], key: &CaseKey) -> bool {
    parser_kinds.iter().any(|kind| key.kind == kind.as_str())
}

fn parser_kinds(args: &PerfGate) -> Result<Vec<String>> {
    match args.kind.as_str() {
        "all" => Ok(vec!["normal".into(), "error".into()]),
        "normal" | "error" => Ok(vec![args.kind.clone()]),
        other => bail!("unsupported perf-gate kind {other:?}; expected normal, error, or all"),
    }
}

fn benchmark_kind(args: &PerfGate) -> Result<String> {
    Ok(parser_kinds(args)?.join(","))
}

fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("BENCHMARK_RESULT missing string field {key}"))
}

fn required_u64(value: &Value, key: &str) -> Result<u64> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("BENCHMARK_RESULT missing integer field {key}"))
}

fn display_case_path(path: &str) -> String {
    let path = Path::new(path);
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| path.to_str().unwrap_or("<invalid path>"))
        .into()
}

fn describe_output(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

fn sanitize_path_component(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
enum CoreImpl {
    Rust,
    C,
}

impl CoreImpl {
    const fn env_value(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::C => "c",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct CaseKey {
    language: String,
    kind: String,
    path: String,
}

#[derive(Clone, Copy, Debug)]
struct Measurement {
    bytes: u64,
    duration_ns: u64,
}

impl Measurement {
    fn speed(self) -> f64 {
        self.bytes as f64 * 1_000_000.0 / self.duration_ns as f64
    }
}

#[derive(Clone, Debug)]
struct Comparison {
    key: CaseKey,
    rust: Measurement,
    c: Measurement,
}

impl Comparison {
    fn rust_slowdown_percent(&self) -> f64 {
        ((self.c.speed() - self.rust.speed()) / self.c.speed()) * 100.0
    }
}

#[derive(Default)]
struct Aggregate {
    cases: usize,
    rust_bytes: u64,
    rust_duration_ns: u64,
    c_bytes: u64,
    c_duration_ns: u64,
}

impl Aggregate {
    fn rust_speed(&self) -> f64 {
        self.rust_bytes as f64 * 1_000_000.0 / self.rust_duration_ns as f64
    }

    fn c_speed(&self) -> f64 {
        self.c_bytes as f64 * 1_000_000.0 / self.c_duration_ns as f64
    }

    fn rust_delta_percent(&self) -> f64 {
        ((self.rust_speed() - self.c_speed()) / self.c_speed()) * 100.0
    }
}
