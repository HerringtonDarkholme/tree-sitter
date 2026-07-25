use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::{benchmark::mebibytes, core_parity, root_dir, PerfGate};

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
    if args.repetitions < 2 {
        bail!("--repetitions must be at least 2 to calculate standard deviation");
    }
    if args.min_sample_time_ms == 0 {
        bail!("--min-sample-time-ms must be greater than zero");
    }

    let root = root_dir();
    core_parity::preflight_c_core_revision(root, &args.c_core_rev)?;
    let c_core_src_dir = core_parity::materialize_c_core(root, &args.c_core_rev)?;

    let mut noisy_cases = Vec::new();
    for language in languages(args) {
        println!("\n==> {language}: Rust core");
        let rust = run_benchmark(root, args, &language, CoreImpl::Rust, None)?;

        println!("==> {language}: C core");
        let c = run_benchmark(root, args, &language, CoreImpl::C, Some(&c_core_src_dir))?;

        let comparisons = compare_measurements(&language, &rust, &c)?;
        print_results(&language, &comparisons);

        for comparison in comparisons {
            let rust_stats = comparison.rust.statistics()?;
            let c_stats = comparison.c.statistics()?;
            if rust_stats.cv_percent > args.max_cv_percent {
                noisy_cases.push(format!(
                    "{} (Rust CV {:.2}%)",
                    display_path(&comparison.key.path),
                    rust_stats.cv_percent
                ));
            }
            if c_stats.cv_percent > args.max_cv_percent {
                noisy_cases.push(format!(
                    "{} (C CV {:.2}%)",
                    display_path(&comparison.key.path),
                    c_stats.cv_percent
                ));
            }
        }
    }

    if !noisy_cases.is_empty() {
        println!("\nNoisy cases:");
        for case in &noisy_cases {
            println!("  {case}");
        }
        bail!(
            "{} measurements exceeded {:.1}% CV",
            noisy_cases.len(),
            args.max_cv_percent
        );
    }

    println!(
        "\nAll measurements stayed within {:.1}% CV.",
        args.max_cv_percent
    );
    Ok(())
}

fn languages(args: &PerfGate) -> Vec<String> {
    if args.languages.is_empty() {
        DEFAULT_LANGUAGES
            .iter()
            .map(|language| (*language).to_owned())
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
        .env("TREE_SITTER_BENCHMARK_KIND_FILTER", "normal")
        .env(
            "TREE_SITTER_BENCHMARK_REPETITION_COUNT",
            args.repetitions.to_string(),
        )
        .env(
            "TREE_SITTER_BENCHMARK_MIN_SAMPLE_TIME_MS",
            args.min_sample_time_ms.to_string(),
        )
        .env(
            "CARGO_TARGET_DIR",
            target_dir_for_core(core_impl, &args.c_core_rev),
        )
        .arg("bench")
        .arg("benchmark")
        .arg("-p")
        .arg("tree-sitter-cli");

    if let Some(src_dir) = c_core_src_dir {
        command.env("TREE_SITTER_C_CORE_SRC_DIR", src_dir);
    }
    if args.offline {
        command.arg("--offline");
    }

    let output = command
        .output()
        .with_context(|| format!("failed to run {command:?}"))?;
    if !output.status.success() {
        bail!(
            "{core_impl:?} benchmark for {language} failed\n{}",
            describe_output(&output)
        );
    }

    let measurements = parse_benchmark_results(&output.stdout)?;
    if measurements.is_empty() {
        bail!("{core_impl:?} benchmark for {language} produced no results");
    }
    Ok(measurements)
}

fn compare_measurements(
    language: &str,
    rust: &BTreeMap<CaseKey, Measurement>,
    c: &BTreeMap<CaseKey, Measurement>,
) -> Result<Vec<Comparison>> {
    let rust_keys = rust.keys().collect::<BTreeSet<_>>();
    let c_keys = c.keys().collect::<BTreeSet<_>>();
    if rust_keys != c_keys {
        let missing_rust = c_keys.difference(&rust_keys).count();
        let missing_c = rust_keys.difference(&c_keys).count();
        bail!(
            "{language} measured different cases: {missing_rust} missing from Rust, {missing_c} missing from C"
        );
    }

    rust.iter()
        .map(|(key, rust_measurement)| {
            let c_measurement = &c[key];
            if rust_measurement.source_bytes != c_measurement.source_bytes
                || rust_measurement.source_hash != c_measurement.source_hash
            {
                bail!("Rust and C measured different input for {}", key.path);
            }
            Ok(Comparison {
                key: key.clone(),
                rust: rust_measurement.clone(),
                c: c_measurement.clone(),
            })
        })
        .collect()
}

fn print_results(language: &str, comparisons: &[Comparison]) {
    println!("\n  {language} throughput (bytes/ms):");
    for comparison in comparisons {
        let rust = comparison.rust.statistics().expect("validated samples");
        let c = comparison.c.statistics().expect("validated samples");
        println!("    {}", display_path(&comparison.key.path));
        println!(
            "      Rust {:>10.1} median  {:>10.1} ± {:>8.1}  CV {:>5.2}%",
            rust.median, rust.mean, rust.stddev, rust.cv_percent
        );
        println!(
            "      C    {:>10.1} median  {:>10.1} ± {:>8.1}  CV {:>5.2}%",
            c.median, c.mean, c.stddev, c.cv_percent
        );
    }

    let rust_peak = comparisons
        .iter()
        .map(|comparison| comparison.rust.peak_rss_bytes)
        .max()
        .unwrap_or(0);
    let c_peak = comparisons
        .iter()
        .map(|comparison| comparison.c.peak_rss_bytes)
        .max()
        .unwrap_or(0);
    println!(
        "    peak RSS: Rust {:.2} MiB, C {:.2} MiB",
        mebibytes(rust_peak),
        mebibytes(c_peak)
    );
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
            language: required_str(&value, "language")?.to_owned(),
            kind: required_str(&value, "kind")?.to_owned(),
            path: required_str(&value, "path")?.to_owned(),
        };
        let measurement = Measurement {
            source_bytes: required_u64(&value, "source_bytes")?,
            source_hash: required_u64(&value, "source_hash")?,
            bytes: required_u64(&value, "bytes")?,
            sample_duration_ns: required_u64_array(&value, "sample_duration_ns")?,
            peak_rss_bytes: required_u64(&value, "peak_rss_bytes")?,
        };
        measurements.insert(key, measurement);
    }
    Ok(measurements)
}

fn required_str<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value[field]
        .as_str()
        .with_context(|| format!("benchmark result has no string field {field:?}"))
}

fn required_u64(value: &Value, field: &str) -> Result<u64> {
    value[field]
        .as_u64()
        .with_context(|| format!("benchmark result has no integer field {field:?}"))
}

fn required_u64_array(value: &Value, field: &str) -> Result<Vec<u64>> {
    value[field]
        .as_array()
        .with_context(|| format!("benchmark result has no array field {field:?}"))?
        .iter()
        .map(|item| {
            item.as_u64()
                .with_context(|| format!("benchmark result field {field:?} is not a u64 array"))
        })
        .collect()
}

fn target_dir_for_core(core_impl: CoreImpl, rev: &str) -> PathBuf {
    std::env::temp_dir()
        .join("tree-sitter-perf-gate-target")
        .join(core_impl.env_value())
        .join(sanitize_path_component(rev))
}

fn sanitize_path_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn display_path(path: &str) -> &str {
    path.rsplit_once("/examples/")
        .map_or(path, |(_, relative)| relative)
}

fn describe_output(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CaseKey {
    language: String,
    kind: String,
    path: String,
}

#[derive(Clone, Debug)]
struct Measurement {
    source_bytes: u64,
    source_hash: u64,
    bytes: u64,
    sample_duration_ns: Vec<u64>,
    peak_rss_bytes: u64,
}

impl Measurement {
    fn statistics(&self) -> Result<Statistics> {
        if self.sample_duration_ns.len() < 2 {
            bail!("a benchmark result needs at least two samples");
        }
        let speeds = self
            .sample_duration_ns
            .iter()
            .map(|duration| self.bytes as f64 * 1_000_000.0 / (*duration).max(1) as f64)
            .collect::<Vec<_>>();
        Ok(Statistics::from_values(&speeds))
    }
}

#[derive(Clone, Copy, Debug)]
struct Statistics {
    median: f64,
    mean: f64,
    stddev: f64,
    cv_percent: f64,
}

impl Statistics {
    fn from_values(values: &[f64]) -> Self {
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let variance = values
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / (values.len() - 1) as f64;
        let stddev = variance.sqrt();
        let cv_percent = if mean == 0.0 {
            0.0
        } else {
            stddev / mean * 100.0
        };

        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let middle = sorted.len() / 2;
        let median = if sorted.len() % 2 == 0 {
            (sorted[middle - 1] + sorted[middle]) / 2.0
        } else {
            sorted[middle]
        };

        Self {
            median,
            mean,
            stddev,
            cv_percent,
        }
    }
}

#[derive(Clone, Debug)]
struct Comparison {
    key: CaseKey,
    rust: Measurement,
    c: Measurement,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistics_report_median_and_sample_standard_deviation() {
        let statistics = Statistics::from_values(&[10.0, 12.0, 14.0]);
        assert_eq!(statistics.median, 12.0);
        assert_eq!(statistics.mean, 12.0);
        assert_eq!(statistics.stddev, 2.0);
        assert!((statistics.cv_percent - 16.666_666_666_7).abs() < 1e-9);
    }

    #[test]
    fn even_sample_count_uses_middle_pair_for_median() {
        let statistics = Statistics::from_values(&[20.0, 10.0, 40.0, 30.0]);
        assert_eq!(statistics.median, 25.0);
    }
}
