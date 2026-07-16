use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
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

const BASELINE_VERSION: u64 = 4;
const BASELINE_PATH: &str = "crates/xtask/perf_baseline.json";
const STABILITY_VERSION: u64 = 4;

pub fn run(args: &PerfGate) -> Result<()> {
    let root = root_dir();
    core_parity::preflight_c_core_revision(root, &args.c_core_rev)?;
    let c_core_src_dir = core_parity::materialize_c_core(root, &args.c_core_rev)?;

    let languages = languages(args);
    let mut all_comparisons = Vec::new();

    if args.measurement_trials == 0 || args.measurement_trials & 1 == 0 {
        bail!("--measurement-trials must be a positive odd number");
    }
    if args.max_sample_attempts == 0 {
        bail!("--max-sample-attempts must be positive");
    }

    for language in &languages {
        println!("\n==> {language}: discovering cases");
        let discovered = run_benchmark(root, args, language, CoreImpl::Rust, None, None)?;
        let (rust_results, c_results, statistics) =
            measure_cases(root, args, language, &discovered, c_core_src_dir.as_path())?;

        let comparisons =
            compare_measurements(args, language, &rust_results, &c_results, &statistics)?;
        print_language_summary(args, language, &comparisons)?;
        all_comparisons.extend(comparisons);
    }

    if all_comparisons.is_empty() {
        bail!("no shared parser benchmark cases were measured");
    }

    print_overall_summary(args, &all_comparisons);
    let stability = StabilitySnapshot::new(args, &all_comparisons);
    print_intra_run_stability(args, &stability);
    let mut stability_failures = intra_run_stability_failures(args, &stability);
    if let Some(path) = &args.stability_reference {
        let reference = StabilitySnapshot::read(path)?;
        stability_failures.extend(compare_stability(args, &reference, &stability)?);
    }
    if let Some(path) = &args.stability_output {
        stability.write(path)?;
    }
    if args.stability_only {
        if !args.report_only && !stability_failures.is_empty() {
            bail!(
                "stability check failed for {} case/metric combinations",
                stability_failures.len()
            );
        }
        println!("stability-only mode: skipping the Rust/C performance baseline");
        return Ok(());
    }
    if args.write_baseline {
        write_baseline(root, args, &all_comparisons)?;
        return Ok(());
    }

    let baseline = load_baseline(root, args)?;
    print_baseline_summary(args, &all_comparisons, &baseline)?;
    enforce_thresholds(args, &all_comparisons, &baseline)?;
    if !args.report_only && !stability_failures.is_empty() {
        bail!(
            "stability check failed for {} case/metric combinations",
            stability_failures.len()
        );
    }

    Ok(())
}

fn measure_cases(
    root: &Path,
    args: &PerfGate,
    language: &str,
    discovered: &BTreeMap<CaseKey, Measurement>,
    c_core_src_dir: &Path,
) -> Result<(
    BTreeMap<CaseKey, Measurement>,
    BTreeMap<CaseKey, Measurement>,
    BTreeMap<CaseKey, TrialStatistics>,
)> {
    let kinds = parser_kinds(args)?;
    let cases = discovered
        .keys()
        .filter(|key| is_parser_case(&kinds, key))
        .collect::<Vec<_>>();
    let mut rust_results = BTreeMap::new();
    let mut c_results = BTreeMap::new();
    let mut statistics = BTreeMap::new();

    for (case_index, key) in cases.iter().enumerate() {
        println!(
            "==> {language}: case {}/{} {}",
            case_index + 1,
            cases.len(),
            display_case_path(&key.path)
        );
        let mut trials = Vec::with_capacity(args.measurement_trials);
        for trial in 0..args.measurement_trials {
            let measure = |core_impl| -> Result<Measurement> {
                let c_src = matches!(core_impl, CoreImpl::C).then_some(c_core_src_dir);
                for attempt in 1..=args.max_sample_attempts {
                    let measurements =
                        run_benchmark(root, args, language, core_impl, c_src, Some(&key.path))?;
                    let measurement = measurements
                        .get(*key)
                        .cloned()
                        .with_context(|| format!("filtered benchmark did not measure {key:?}"))?;
                    let cv = measurement.throughput_statistics().cv_percent;
                    if cv <= args.max_intra_cv_percent {
                        return Ok(measurement);
                    }
                    println!(
                        "    reject {core_impl:?} sample attempt {attempt}: intra-process CV {cv:.2}%"
                    );
                }
                bail!(
                    "{core_impl:?} benchmark for {key:?} exceeded {:.2}% CV on all {} attempts",
                    args.max_intra_cv_percent,
                    args.max_sample_attempts,
                )
            };
            if trial & 1 == 0 {
                trials.push((measure(CoreImpl::Rust)?, measure(CoreImpl::C)?));
            } else {
                let c = measure(CoreImpl::C)?;
                trials.push((measure(CoreImpl::Rust)?, c));
            }
        }
        let (rust, c, trial_statistics) = median_pair(key, &mut trials)?;
        rust_results.insert((*key).clone(), rust);
        c_results.insert((*key).clone(), c);
        statistics.insert((*key).clone(), trial_statistics);
    }

    Ok((rust_results, c_results, statistics))
}

fn median_pair(
    key: &CaseKey,
    trials: &mut [(Measurement, Measurement)],
) -> Result<(Measurement, Measurement, TrialStatistics)> {
    let Some(first) = trials.first() else {
        bail!("no benchmark trials were measured for {key:?}");
    };
    if trials.iter().any(|(rust, c)| {
        rust.source_bytes != first.0.source_bytes
            || c.source_bytes != first.1.source_bytes
            || rust.source_hash != first.0.source_hash
            || c.source_hash != first.1.source_hash
            || rust.sample_duration_ns.len() != first.0.sample_duration_ns.len()
            || c.sample_duration_ns.len() != first.1.sample_duration_ns.len()
    }) {
        bail!("benchmark trials measured different byte counts for {key:?}");
    }
    let rust_peak_rss = trials
        .iter()
        .map(|(rust, _)| rust.peak_rss_bytes)
        .max()
        .unwrap_or(0);
    let c_peak_rss = trials
        .iter()
        .map(|(_, c)| c.peak_rss_bytes)
        .max()
        .unwrap_or(0);
    let process_statistics = TrialStatistics::from_process_means(trials);
    println!(
        "    process-mean CV Rust {:>5.2}%  C {:>5.2}%  paired {:>5.2}%",
        process_statistics.rust_throughput.cv_percent,
        process_statistics.c_throughput.cv_percent,
        process_statistics.throughput_ratio.cv_percent,
    );
    trials.sort_by(|(rust_a, c_a), (rust_b, c_b)| {
        let ratio_a = rust_a.speed() / c_a.speed();
        let ratio_b = rust_b.speed() / c_b.speed();
        ratio_a
            .partial_cmp(&ratio_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let statistics = TrialStatistics::from_trial_samples(trials)?;
    let (mut rust, mut c) = trials[trials.len() / 2].clone();
    rust.peak_rss_bytes = rust_peak_rss;
    c.peak_rss_bytes = c_peak_rss;
    println!(
        "    intra-run CV     Rust {:>5.2}%  C {:>5.2}%  paired {:>5.2}%",
        statistics.rust_throughput.cv_percent,
        statistics.c_throughput.cv_percent,
        statistics.throughput_ratio.cv_percent,
    );
    Ok((rust, c, statistics))
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
    case_filter: Option<&str>,
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
        .env(
            "TREE_SITTER_BENCHMARK_MIN_SOURCE_BYTES",
            args.min_case_bytes.to_string(),
        )
        .env(
            "TREE_SITTER_BENCHMARK_MIN_SAMPLE_TIME_MS",
            args.min_sample_time_ms.to_string(),
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

    if let Some(case_filter) = case_filter {
        command.env("TREE_SITTER_BENCHMARK_EXAMPLE_PATH", case_filter);
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
            source_bytes: required_u64(&value, "source_bytes")?,
            source_hash: required_u64(&value, "source_hash")?,
            bytes: required_u64(&value, "bytes")?,
            duration_ns: required_u64(&value, "duration_ns")?.max(1),
            sample_duration_ns: required_u64_array(&value, "sample_duration_ns")?,
            peak_rss_bytes: required_u64(&value, "peak_rss_bytes")?,
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
    statistics: &BTreeMap<CaseKey, TrialStatistics>,
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
            c_results.get(key).map(|c| {
                Ok(Comparison {
                    key: key.clone(),
                    rust: rust.clone(),
                    c: c.clone(),
                    statistics: *statistics
                        .get(key)
                        .with_context(|| format!("missing trial statistics for {key:?}"))?,
                })
            })
        })
        .collect::<Result<Vec<_>>>()?;

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
    let rust_peak_rss = comparisons
        .iter()
        .map(|comparison| comparison.rust.peak_rss_bytes)
        .max()
        .unwrap_or(0);
    let c_peak_rss = comparisons
        .iter()
        .map(|comparison| comparison.c.peak_rss_bytes)
        .max()
        .unwrap_or(0);
    println!(
        "    peak RSS       Rust {:>10.1} MiB  C {:>10.1} MiB  delta {:+6.2}% (report only)",
        mebibytes(rust_peak_rss),
        mebibytes(c_peak_rss),
        percent_delta(rust_peak_rss, c_peak_rss),
    );
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
    println!(
        "    peak RSS       Rust {:>10.1} MiB  C {:>10.1} MiB  delta {:+6.2}% (report only)",
        mebibytes(aggregate.rust_peak_rss_bytes),
        mebibytes(aggregate.c_peak_rss_bytes),
        percent_delta(aggregate.rust_peak_rss_bytes, aggregate.c_peak_rss_bytes,),
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

fn print_intra_run_stability(args: &PerfGate, snapshot: &StabilitySnapshot) {
    let failures = intra_run_stability_failures(args, snapshot);
    println!("\n==> Intra-run stability");
    println!(
        "    {} cases, {} alternating pairs per case, maximum CV {:.2}%",
        snapshot.cases.len(),
        snapshot.measurement_trials,
        args.max_intra_cv_percent,
    );
    if failures.is_empty() {
        println!("    all Rust, C, and paired-ratio distributions are within the CV limit");
    } else {
        println!("    {} distributions exceed the CV limit:", failures.len());
        for failure in failures.iter().take(20) {
            println!("    - {failure}");
        }
    }
}

fn intra_run_stability_failures(args: &PerfGate, snapshot: &StabilitySnapshot) -> Vec<String> {
    let mut failures = Vec::new();
    for (case, measurement) in &snapshot.cases {
        for (metric, statistics) in measurement.statistics.throughput_distributions() {
            if statistics.cv_percent > args.max_intra_cv_percent {
                failures.push(format!("{case} {metric}: CV {:.2}%", statistics.cv_percent));
            }
        }
    }
    failures
}

fn compare_stability(
    args: &PerfGate,
    reference: &StabilitySnapshot,
    current: &StabilitySnapshot,
) -> Result<Vec<String>> {
    if reference.repetitions != current.repetitions
        || reference.measurement_trials != current.measurement_trials
        || reference.min_case_bytes != current.min_case_bytes
        || reference.min_sample_time_ms != current.min_sample_time_ms
        || reference.max_sample_attempts != current.max_sample_attempts
    {
        bail!("stability snapshots use different measurement settings");
    }
    if reference.cases.keys().collect::<Vec<_>>() != current.cases.keys().collect::<Vec<_>>() {
        bail!("stability snapshots contain different benchmark cases");
    }

    let mut failures = Vec::new();
    println!("\n==> Inter-run stability");
    for (case, current_case) in &current.cases {
        let reference_case = &reference.cases[case];
        if current_case.source_bytes != reference_case.source_bytes
            || current_case.source_hash != reference_case.source_hash
        {
            bail!("stability fixture {case} changed between runs");
        }
        for ((metric, reference_stats), (_, current_stats)) in reference_case
            .statistics
            .throughput_distributions()
            .into_iter()
            .zip(current_case.statistics.throughput_distributions())
        {
            if !reference_stats.intervals_overlap(current_stats) {
                failures.push(format!(
                    "{case} {metric}: means {:.3} and {:.3} differ by more than one stddev from each run ({:.3} + {:.3})",
                    reference_stats.mean,
                    current_stats.mean,
                    reference_stats.stddev,
                    current_stats.stddev,
                ));
            }
            let cv_delta = (reference_stats.cv_percent - current_stats.cv_percent).abs();
            if cv_delta > args.max_stddev_cv_delta_percent {
                failures.push(format!(
                    "{case} {metric}: CV changed {:.2} points ({:.2}% -> {:.2}%)",
                    cv_delta, reference_stats.cv_percent, current_stats.cv_percent,
                ));
            }
        }
    }
    if failures.is_empty() {
        println!(
            "    every mean±stddev interval overlaps; CV changes are at most {:.2} points",
            args.max_stddev_cv_delta_percent
        );
    } else {
        println!("    {} inter-run stability failures:", failures.len());
        for failure in failures.iter().take(20) {
            println!("    - {failure}");
        }
    }
    Ok(failures)
}

fn enforce_thresholds(
    args: &PerfGate,
    comparisons: &[Comparison],
    baseline: &BTreeMap<String, BaselineCase>,
) -> Result<()> {
    if args.report_only {
        println!("perf gate report-only mode: not enforcing thresholds");
        return Ok(());
    }

    let overall_delta = baseline_delta(comparisons, baseline)?;
    let regressions = baseline_regressions(args, comparisons, baseline)?;

    if overall_delta < -args.max_overall_regression_percent || !regressions.is_empty() {
        bail!(
            "perf gate failed: baseline-normalized Rust delta {overall_delta:+.2}% (minimum {:+.2}%), {} per-case regressions above {:.2}%",
            -args.max_overall_regression_percent,
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

fn baseline_regressions<'a>(
    args: &PerfGate,
    comparisons: &'a [Comparison],
    baseline: &BTreeMap<String, BaselineCase>,
) -> Result<Vec<(&'a Comparison, f64)>> {
    let mut regressions = Vec::new();
    for comparison in comparisons {
        if comparison.rust.source_bytes < args.min_enforced_case_bytes {
            continue;
        }
        let slowdown = baseline_slowdown(comparison, baseline)?;
        if slowdown > args.max_regression_percent {
            regressions.push((comparison, slowdown));
        }
    }
    regressions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(regressions)
}

fn print_baseline_summary(
    args: &PerfGate,
    comparisons: &[Comparison],
    baseline: &BTreeMap<String, BaselineCase>,
) -> Result<()> {
    let delta = baseline_delta(comparisons, baseline)?;
    println!("\n==> Change from checked-in Rust/C ratio baseline");
    println!("    overall normalized delta {delta:+.2}%");
    let regressions = baseline_regressions(args, comparisons, baseline)?;
    if regressions.is_empty() {
        println!(
            "    no baseline-normalized per-case regressions above {:.2}% for fixtures at least {} bytes",
            args.max_regression_percent,
            args.min_enforced_case_bytes,
        );
    } else {
        println!(
            "    baseline-normalized per-case regressions above {:.2}%:",
            args.max_regression_percent
        );
        for (comparison, slowdown) in regressions.iter().take(10) {
            println!(
                "    - {} {} {}: slowdown {:.2}%",
                comparison.key.language,
                comparison.key.kind,
                display_case_path(&comparison.key.path),
                slowdown,
            );
        }
    }
    Ok(())
}

fn baseline_delta(
    comparisons: &[Comparison],
    baseline: &BTreeMap<String, BaselineCase>,
) -> Result<f64> {
    let mut actual_duration = 0.0;
    let mut expected_duration = 0.0;
    for comparison in comparisons {
        let weight = comparison.rust.source_bytes as f64;
        actual_duration += weight / comparison.rust.speed();
        expected_duration +=
            weight / (comparison.c.speed() * baseline_ratio(comparison, baseline)?);
    }
    Ok((expected_duration / actual_duration - 1.0) * 100.0)
}

fn baseline_slowdown(
    comparison: &Comparison,
    baseline: &BTreeMap<String, BaselineCase>,
) -> Result<f64> {
    let current_ratio = comparison.rust.speed() / comparison.c.speed();
    Ok((baseline_ratio(comparison, baseline)? / current_ratio - 1.0) * 100.0)
}

fn baseline_ratio(
    comparison: &Comparison,
    baseline: &BTreeMap<String, BaselineCase>,
) -> Result<f64> {
    let key = baseline_key(&comparison.key);
    let baseline = baseline
        .get(&key)
        .with_context(|| format!("performance baseline is missing case {key}"))?;
    if baseline.source_bytes != comparison.rust.source_bytes
        || baseline.source_hash != comparison.rust.source_hash
    {
        bail!(
            "performance fixture {key} changed; rewrite the baseline as an explicit corpus update"
        );
    }
    Ok(baseline.rust_c_throughput_ratio)
}

fn baseline_key(key: &CaseKey) -> String {
    format!(
        "{}|{}|{}",
        key.language,
        key.kind,
        display_case_path(&key.path)
    )
}

fn load_baseline(root: &Path, args: &PerfGate) -> Result<BTreeMap<String, BaselineCase>> {
    let path = root.join(BASELINE_PATH);
    let baseline: BaselineDocument = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?,
    )?;
    if baseline.version != BASELINE_VERSION {
        bail!("unsupported performance baseline version");
    }
    if baseline.min_case_bytes != args.min_case_bytes as u64 {
        bail!(
            "performance baseline uses a different --min-case-bytes value; run with {} or explicitly rewrite the baseline",
            baseline.min_case_bytes
        );
    }
    if baseline.measurement_trials != args.measurement_trials as u64 {
        bail!("performance baseline uses a different --measurement-trials value");
    }
    if baseline.repetitions != args.repetitions as u64 {
        bail!("performance baseline uses a different --repetitions value");
    }
    if baseline.min_sample_time_ms != args.min_sample_time_ms {
        bail!("performance baseline uses a different --min-sample-time-ms value");
    }
    if baseline.max_sample_attempts != args.max_sample_attempts as u64 {
        bail!("performance baseline uses a different --max-sample-attempts value");
    }
    if baseline.min_enforced_case_bytes != args.min_enforced_case_bytes {
        bail!("performance baseline uses a different --min-enforced-case-bytes value");
    }
    if baseline.c_core_rev != args.c_core_rev {
        bail!("performance baseline uses a different C core revision");
    }
    Ok(baseline.cases)
}

fn write_baseline(root: &Path, args: &PerfGate, comparisons: &[Comparison]) -> Result<()> {
    if !args.languages.is_empty() || args.kind != "normal" {
        bail!("the checked-in baseline must contain the default languages and normal cases");
    }

    let mut cases = BTreeMap::new();
    for comparison in comparisons {
        let key = baseline_key(&comparison.key);
        let baseline = BaselineCase {
            source_bytes: comparison.rust.source_bytes,
            source_hash: comparison.rust.source_hash,
            rust_c_throughput_ratio: comparison.rust.speed() / comparison.c.speed(),
        };
        if cases.insert(key.clone(), baseline).is_some() {
            bail!("duplicate performance baseline key {key}");
        }
    }
    let path = root.join(BASELINE_PATH);
    let baseline = BaselineDocument {
        version: BASELINE_VERSION,
        min_case_bytes: args.min_case_bytes as u64,
        measurement_trials: args.measurement_trials as u64,
        repetitions: args.repetitions as u64,
        min_sample_time_ms: args.min_sample_time_ms,
        max_sample_attempts: args.max_sample_attempts as u64,
        min_enforced_case_bytes: args.min_enforced_case_bytes,
        c_core_rev: args.c_core_rev.clone(),
        cases,
    };
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&baseline)?),
    )
    .with_context(|| format!("failed to write {}", path.display()))?;
    println!("wrote performance baseline to {}", path.display());
    Ok(())
}

fn aggregate<'a>(comparisons: impl Iterator<Item = &'a Comparison>) -> Option<Aggregate> {
    let mut aggregate = Aggregate::default();
    for comparison in comparisons {
        aggregate.cases += 1;
        aggregate.rust_bytes += comparison.rust.bytes;
        aggregate.rust_duration_ns += comparison.rust.duration_ns;
        aggregate.c_bytes += comparison.c.bytes;
        aggregate.c_duration_ns += comparison.c.duration_ns;
        aggregate.rust_peak_rss_bytes = aggregate
            .rust_peak_rss_bytes
            .max(comparison.rust.peak_rss_bytes);
        aggregate.c_peak_rss_bytes = aggregate.c_peak_rss_bytes.max(comparison.c.peak_rss_bytes);
    }
    (aggregate.cases != 0).then_some(aggregate)
}

fn percent_delta(value: u64, baseline: u64) -> f64 {
    if baseline == 0 {
        return 0.0;
    }
    ((value as f64 - baseline as f64) / baseline as f64) * 100.0
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

fn required_u64_array(value: &Value, key: &str) -> Result<Vec<u64>> {
    value
        .get(key)
        .and_then(Value::as_array)
        .with_context(|| format!("BENCHMARK_RESULT missing array field {key}"))?
        .iter()
        .map(|value| {
            value
                .as_u64()
                .with_context(|| format!("BENCHMARK_RESULT field {key} contains a non-integer"))
        })
        .collect()
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

#[derive(Clone, Debug)]
struct Measurement {
    source_bytes: u64,
    source_hash: u64,
    bytes: u64,
    duration_ns: u64,
    sample_duration_ns: Vec<u64>,
    peak_rss_bytes: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct DistributionStatistics {
    mean: f64,
    stddev: f64,
    cv_percent: f64,
}

impl DistributionStatistics {
    fn from_values(values: &[f64]) -> Self {
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let variance = if values.len() > 1 {
            values
                .iter()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>()
                / (values.len() - 1) as f64
        } else {
            0.0
        };
        let stddev = variance.sqrt();
        let cv_percent = if mean == 0.0 {
            0.0
        } else {
            stddev / mean.abs() * 100.0
        };
        Self {
            mean,
            stddev,
            cv_percent,
        }
    }

    fn intervals_overlap(self, other: Self) -> bool {
        (self.mean - other.mean).abs() <= self.stddev + other.stddev
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct TrialStatistics {
    rust_throughput: DistributionStatistics,
    c_throughput: DistributionStatistics,
    throughput_ratio: DistributionStatistics,
}

impl TrialStatistics {
    fn from_process_means(trials: &[(Measurement, Measurement)]) -> Self {
        let rust = trials
            .iter()
            .map(|(rust, _)| rust.speed())
            .collect::<Vec<_>>();
        let c = trials.iter().map(|(_, c)| c.speed()).collect::<Vec<_>>();
        let ratio = trials
            .iter()
            .map(|(rust, c)| rust.speed() / c.speed())
            .collect::<Vec<_>>();
        Self {
            rust_throughput: DistributionStatistics::from_values(&rust),
            c_throughput: DistributionStatistics::from_values(&c),
            throughput_ratio: DistributionStatistics::from_values(&ratio),
        }
    }

    fn from_trial_samples(trials: &[(Measurement, Measurement)]) -> Result<Self> {
        let mut rust_samples = Vec::new();
        let mut c_samples = Vec::new();
        let mut ratio_samples = Vec::new();
        for (rust, c) in trials {
            if rust.sample_duration_ns.is_empty()
                || rust.sample_duration_ns.len() != c.sample_duration_ns.len()
            {
                bail!("paired benchmark processes emitted different sample counts");
            }
            let rust_trial = rust.sample_speeds();
            let c_trial = c.sample_speeds();
            ratio_samples.extend(rust_trial.iter().zip(&c_trial).map(|(rust, c)| rust / c));
            rust_samples.extend(rust_trial);
            c_samples.extend(c_trial);
        }
        Ok(Self {
            rust_throughput: DistributionStatistics::from_values(&rust_samples),
            c_throughput: DistributionStatistics::from_values(&c_samples),
            throughput_ratio: DistributionStatistics::from_values(&ratio_samples),
        })
    }

    fn throughput_distributions(self) -> [(&'static str, DistributionStatistics); 2] {
        [
            ("Rust throughput", self.rust_throughput),
            ("C throughput", self.c_throughput),
        ]
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct StabilityCase {
    source_bytes: u64,
    source_hash: u64,
    statistics: TrialStatistics,
}

#[derive(Debug, Deserialize, Serialize)]
struct StabilitySnapshot {
    version: u64,
    repetitions: usize,
    measurement_trials: usize,
    min_case_bytes: usize,
    min_sample_time_ms: u64,
    max_sample_attempts: u64,
    cases: BTreeMap<String, StabilityCase>,
}

impl StabilitySnapshot {
    fn new(args: &PerfGate, comparisons: &[Comparison]) -> Self {
        let cases = comparisons
            .iter()
            .map(|comparison| {
                (
                    baseline_key(&comparison.key),
                    StabilityCase {
                        source_bytes: comparison.rust.source_bytes,
                        source_hash: comparison.rust.source_hash,
                        statistics: comparison.statistics,
                    },
                )
            })
            .collect();
        Self {
            version: STABILITY_VERSION,
            repetitions: args.repetitions,
            measurement_trials: args.measurement_trials,
            min_case_bytes: args.min_case_bytes,
            min_sample_time_ms: args.min_sample_time_ms,
            max_sample_attempts: args.max_sample_attempts as u64,
            cases,
        }
    }

    fn read(path: &Path) -> Result<Self> {
        let snapshot: Self = serde_json::from_slice(
            &fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
        )?;
        if snapshot.version != STABILITY_VERSION {
            bail!("unsupported stability snapshot version");
        }
        Ok(snapshot)
    }

    fn write(&self, path: &Path) -> Result<()> {
        fs::write(path, format!("{}\n", serde_json::to_string_pretty(self)?))
            .with_context(|| format!("failed to write {}", path.display()))?;
        println!("wrote stability statistics to {}", path.display());
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct BaselineCase {
    source_bytes: u64,
    source_hash: u64,
    rust_c_throughput_ratio: f64,
}

#[derive(Debug, Deserialize, Serialize)]
struct BaselineDocument {
    version: u64,
    min_case_bytes: u64,
    measurement_trials: u64,
    repetitions: u64,
    min_sample_time_ms: u64,
    max_sample_attempts: u64,
    min_enforced_case_bytes: u64,
    c_core_rev: String,
    cases: BTreeMap<String, BaselineCase>,
}

impl Measurement {
    fn speed(&self) -> f64 {
        self.bytes as f64 * 1_000_000.0 / self.duration_ns as f64
    }

    fn sample_speeds(&self) -> Vec<f64> {
        self.sample_duration_ns
            .iter()
            .map(|duration| self.bytes as f64 * 1_000_000.0 / *duration as f64)
            .collect()
    }

    fn throughput_statistics(&self) -> DistributionStatistics {
        DistributionStatistics::from_values(&self.sample_speeds())
    }
}

#[derive(Clone, Debug)]
struct Comparison {
    key: CaseKey,
    rust: Measurement,
    c: Measurement,
    statistics: TrialStatistics,
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
    rust_peak_rss_bytes: u64,
    c_peak_rss_bytes: u64,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> CaseKey {
        CaseKey {
            language: "test".into(),
            kind: "normal".into(),
            path: "/repo/crates/cli/benches/examples/test/case.txt".into(),
        }
    }

    fn measurement(duration_ns: u64, peak_rss_bytes: u64) -> Measurement {
        Measurement {
            source_bytes: 128,
            source_hash: 42,
            bytes: 1024,
            duration_ns,
            sample_duration_ns: vec![duration_ns, duration_ns + 1],
            peak_rss_bytes,
        }
    }

    #[test]
    fn paired_median_keeps_one_observed_pair_and_maximum_rss() {
        let mut trials = [
            (measurement(120, 10), measurement(100, 20)),
            (measurement(80, 30), measurement(100, 40)),
            (measurement(100, 50), measurement(100, 60)),
        ];

        let (rust, c, statistics) = median_pair(&key(), &mut trials).unwrap();

        assert_eq!(rust.duration_ns, 100);
        assert_eq!(c.duration_ns, 100);
        assert_eq!(rust.peak_rss_bytes, 50);
        assert_eq!(c.peak_rss_bytes, 60);
        assert!(statistics.rust_throughput.stddev > 0.0);
    }

    #[test]
    fn baseline_rejects_changed_fixture_contents() {
        let key = key();
        let comparison = Comparison {
            key: key.clone(),
            rust: measurement(100, 0),
            c: measurement(100, 0),
            statistics: TrialStatistics::from_trial_samples(&[(
                measurement(100, 0),
                measurement(100, 0),
            )])
            .unwrap(),
        };
        let baseline = BTreeMap::from([(
            baseline_key(&key),
            BaselineCase {
                source_bytes: 128,
                source_hash: 99,
                rust_c_throughput_ratio: 1.0,
            },
        )]);

        let error = baseline_ratio(&comparison, &baseline).unwrap_err();
        assert!(error
            .to_string()
            .contains("fixture test|normal|case.txt changed"));
    }

    #[test]
    fn distribution_uses_sample_standard_deviation() {
        let statistics = DistributionStatistics::from_values(&[10.0, 12.0, 14.0]);
        assert_eq!(statistics.mean, 12.0);
        assert_eq!(statistics.stddev, 2.0);
        assert!((statistics.cv_percent - 16.666_666_666_7).abs() < 1e-9);
    }
}
