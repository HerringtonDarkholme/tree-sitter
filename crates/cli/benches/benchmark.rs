use std::{
    collections::BTreeMap,
    env, fs,
    mem::MaybeUninit,
    path::{Path, PathBuf},
    str,
    sync::LazyLock,
    time::Duration,
};

#[cfg(not(unix))]
use std::time::Instant;

use anyhow::Context;
use log::info;
use serde_json::json;
use tree_sitter::{Language, Parser, Query};
use tree_sitter_loader::{CompileConfig, Loader};

include!("../src/tests/helpers/dirs.rs");

static LANGUAGE_FILTER: LazyLock<Option<String>> =
    LazyLock::new(|| env::var("TREE_SITTER_BENCHMARK_LANGUAGE_FILTER").ok());
static EXAMPLE_FILTER: LazyLock<Option<String>> =
    LazyLock::new(|| env::var("TREE_SITTER_BENCHMARK_EXAMPLE_FILTER").ok());
static KIND_FILTER: LazyLock<Option<Vec<String>>> = LazyLock::new(|| {
    env::var("TREE_SITTER_BENCHMARK_KIND_FILTER")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|kind| !kind.is_empty())
                .map(str::to_owned)
                .collect()
        })
});
static REPETITION_COUNT: LazyLock<usize> = LazyLock::new(|| {
    env::var("TREE_SITTER_BENCHMARK_REPETITION_COUNT").map_or(5, |s| s.parse::<usize>().unwrap())
});
static MIN_SAMPLE_TIME: LazyLock<std::time::Duration> = LazyLock::new(|| {
    std::time::Duration::from_millis(
        env::var("TREE_SITTER_BENCHMARK_MIN_SAMPLE_TIME_MS")
            .map_or(0, |s| s.parse::<u64>().unwrap()),
    )
});
static ERROR_CASE_LIMIT: LazyLock<Option<usize>> = LazyLock::new(|| {
    env::var("TREE_SITTER_BENCHMARK_ERROR_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
});
static TEST_LOADER: LazyLock<Loader> =
    LazyLock::new(|| Loader::with_parser_lib_path(SCRATCH_DIR.clone()));

#[allow(clippy::type_complexity)]
static EXAMPLE_AND_QUERY_PATHS_BY_LANGUAGE_DIR: LazyLock<
    BTreeMap<PathBuf, (Vec<PathBuf>, Vec<PathBuf>)>,
> = LazyLock::new(|| {
    fn collect_files(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir).map_or_else(
            |_| Vec::new(),
            |entries| {
                entries
                    .filter_map(|entry| {
                        let path = entry.ok()?.path();
                        path.is_file().then_some(path)
                    })
                    .collect()
            },
        )
    }

    fn collect_local_and_parent_files(dir: &Path, child: &str) -> Vec<PathBuf> {
        let mut paths = collect_files(&dir.join(child));
        if let Some(parent) = dir.parent() {
            if parent.starts_with(GRAMMARS_DIR.as_path()) {
                paths.extend(collect_files(&parent.join(child)));
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }

    fn collect_benchmark_examples(relative_path: &Path) -> Vec<PathBuf> {
        collect_files(
            &ROOT_DIR
                .join("crates")
                .join("cli")
                .join("benches")
                .join("examples")
                .join(relative_path),
        )
    }

    fn process_dir(result: &mut BTreeMap<PathBuf, (Vec<PathBuf>, Vec<PathBuf>)>, dir: &Path) {
        if dir.join("grammar.js").exists() {
            let relative_path = dir.strip_prefix(GRAMMARS_DIR.as_path()).unwrap();
            let (example_paths, query_paths) = result.entry(relative_path.to_owned()).or_default();

            // Performance inputs are repository-owned snapshots. Do not make
            // benchmark results depend on ignored grammar examples or sibling
            // repositories that happen to exist on one developer's machine.
            example_paths.extend(collect_benchmark_examples(relative_path));
            query_paths.extend(collect_local_and_parent_files(dir, "queries"));
            example_paths.sort();
            example_paths.dedup();
            query_paths.sort();
            query_paths.dedup();
        } else {
            for entry in fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap().path();
                if entry.is_dir() {
                    process_dir(result, &entry);
                }
            }
        }
    }

    let mut result = BTreeMap::new();
    process_dir(&mut result, &GRAMMARS_DIR);
    result
});

fn should_run_example(path: &Path) -> bool {
    EXAMPLE_FILTER
        .as_ref()
        .is_none_or(|filter| path.to_string_lossy().contains(filter))
}

fn main() {
    tree_sitter_cli::logger::init();

    let max_path_length = EXAMPLE_AND_QUERY_PATHS_BY_LANGUAGE_DIR
        .values()
        .flat_map(|(e, q)| {
            e.iter()
                .chain(q.iter())
                .map(|s| s.file_name().unwrap().to_str().unwrap().len())
        })
        .max()
        .unwrap_or(0);

    info!("Benchmarking with {} repetitions", *REPETITION_COUNT);

    let mut parser = Parser::new();
    let mut all_normal_speeds = Vec::new();
    let mut all_error_speeds = Vec::new();

    for (language_path, (example_paths, query_paths)) in
        EXAMPLE_AND_QUERY_PATHS_BY_LANGUAGE_DIR.iter()
    {
        let language_name = language_path.file_name().unwrap().to_str().unwrap();

        if let Some(filter) = LANGUAGE_FILTER.as_ref() {
            if language_name != filter.as_str() {
                continue;
            }
        }

        info!("\nLanguage: {language_name}");
        let language = get_language(language_path);
        parser.set_language(&language).unwrap();

        if should_run_kind("query") {
            info!("  Constructing Queries");
            for path in query_paths {
                if !should_run_example(path) {
                    continue;
                }

                parse(language_name, "query", path, max_path_length, |source| {
                    Query::new(&language, str::from_utf8(source).unwrap())
                        .with_context(|| format!("Query file path: {}", path.display()))
                        .expect("Failed to parse query");
                });
            }
        }

        let mut normal_speeds = Vec::new();
        if should_run_kind("normal") {
            info!("  Parsing Valid Code:");
            for example_path in example_paths {
                if !should_run_example(example_path) {
                    continue;
                }

                let source = fs::read(example_path)
                    .with_context(|| format!("Failed to read {}", example_path.display()))
                    .unwrap();
                let tree = parser.parse(&source, None).expect("Failed to parse");
                assert!(
                    !tree.root_node().has_error(),
                    "normal benchmark fixture has parse errors: {}\n{}",
                    example_path.display(),
                    tree.root_node().to_sexp()
                );

                normal_speeds.push(parse(
                    language_name,
                    "normal",
                    example_path,
                    max_path_length,
                    |code| {
                        parser.parse(code, None).expect("Failed to parse");
                    },
                ));
            }
        }

        let mut error_speeds = Vec::new();
        if should_run_kind("error") {
            info!("  Parsing Invalid Code (mismatched languages):");
            for (other_language_path, (example_paths, _)) in
                EXAMPLE_AND_QUERY_PATHS_BY_LANGUAGE_DIR.iter()
            {
                if other_language_path != language_path {
                    for example_path in example_paths
                        .iter()
                        .take((*ERROR_CASE_LIMIT).unwrap_or(usize::MAX))
                    {
                        if !should_run_example(example_path) {
                            continue;
                        }

                        error_speeds.push(parse(
                            language_name,
                            "error",
                            example_path,
                            max_path_length,
                            |code| {
                                parser.parse(code, None).expect("Failed to parse");
                            },
                        ));
                    }
                }
            }
        }

        if let Some((average_normal, worst_normal)) = aggregate(&normal_speeds) {
            info!("  Average Speed (normal): {average_normal} bytes/ms");
            info!("  Worst Speed (normal):   {worst_normal} bytes/ms");
        }

        if let Some((average_error, worst_error)) = aggregate(&error_speeds) {
            info!("  Average Speed (errors): {average_error} bytes/ms");
            info!("  Worst Speed (errors):   {worst_error} bytes/ms");
        }

        all_normal_speeds.extend(normal_speeds);
        all_error_speeds.extend(error_speeds);
    }

    info!("\n  Overall");
    if let Some((average_normal, worst_normal)) = aggregate(&all_normal_speeds) {
        info!("  Average Speed (normal): {average_normal} bytes/ms");
        info!("  Worst Speed (normal):   {worst_normal} bytes/ms");
    }

    if let Some((average_error, worst_error)) = aggregate(&all_error_speeds) {
        info!("  Average Speed (errors): {average_error} bytes/ms");
        info!("  Worst Speed (errors):   {worst_error} bytes/ms");
    }
    info!("");
}

fn should_run_kind(kind: &str) -> bool {
    match KIND_FILTER.as_ref() {
        Some(filter) => filter.iter().any(|item| item == kind || item == "all"),
        None => true,
    }
}

fn aggregate(speeds: &[usize]) -> Option<(usize, usize)> {
    if speeds.is_empty() {
        return None;
    }
    let mut total = 0;
    let mut max = usize::MAX;
    for speed in speeds.iter().copied() {
        total += speed;
        if speed < max {
            max = speed;
        }
    }
    Some((total / speeds.len(), max))
}

fn parse(
    language: &str,
    kind: &str,
    path: &Path,
    max_path_length: usize,
    mut action: impl FnMut(&[u8]),
) -> usize {
    let source_code = fs::read(path)
        .with_context(|| format!("Failed to read {}", path.display()))
        .unwrap();
    let source_hash = source_hash(&source_code);
    let parses_per_repetition =
        calibrated_parses_per_repetition(&source_code, *MIN_SAMPLE_TIME, &mut action);
    let mut sample_duration_ns = Vec::with_capacity(*REPETITION_COUNT);
    for _ in 0..*REPETITION_COUNT {
        let time = BenchmarkTimer::start();
        for _ in 0..parses_per_repetition {
            action(&source_code);
        }
        sample_duration_ns.push(
            u64::try_from(time.elapsed().as_nanos())
                .unwrap_or(u64::MAX)
                .max(1),
        );
    }
    let duration_ns =
        sample_duration_ns.iter().sum::<u64>() / u64::try_from(sample_duration_ns.len()).unwrap();
    let measured_bytes = source_code.len() as u64 * parses_per_repetition as u64;
    let speed = (measured_bytes * 1_000_000) / duration_ns;
    let peak_rss_bytes = peak_rss_bytes();
    info!(
        "    {:max_path_length$}\ttime {:>7.2} ms\t\tspeed {speed:>6} bytes/ms",
        path.file_name().unwrap().to_str().unwrap(),
        (duration_ns as f64) / 1e6,
    );
    println!(
        "BENCHMARK_RESULT {}",
        json!({
            "language": language,
            "kind": kind,
            "path": path.display().to_string(),
            "source_bytes": source_code.len() as u64,
            "source_hash": source_hash,
            "bytes": measured_bytes,
            "duration_ns": duration_ns,
            "sample_duration_ns": sample_duration_ns,
            "peak_rss_bytes": peak_rss_bytes,
            "speed_bytes_per_ms": speed,
        })
    );
    speed as usize
}

/// Stable FNV-1a fingerprint used to reject stale performance baselines.
fn source_hash(source: &[u8]) -> u64 {
    source.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

/// Calibrate samples to a useful duration before recording them.
/// Three untimed parses warm allocator and parser state and make the estimate
/// less sensitive to one cold call.
fn calibrated_parses_per_repetition(
    source: &[u8],
    minimum_sample_time: std::time::Duration,
    action: &mut impl FnMut(&[u8]),
) -> usize {
    if minimum_sample_time.is_zero() {
        return 1;
    }

    const WARMUP_PARSES: usize = 3;
    for _ in 0..WARMUP_PARSES {
        action(source);
    }

    const CALIBRATION_PARSES: usize = 3;
    let started = BenchmarkTimer::start();
    for _ in 0..CALIBRATION_PARSES {
        action(source);
    }
    let nanos_per_parse = started.elapsed().as_nanos().max(1) / CALIBRATION_PARSES as u128;
    let time_floor = minimum_sample_time
        .as_nanos()
        .div_ceil(nanos_per_parse)
        .min(usize::MAX as u128) as usize;
    time_floor.max(1)
}

/// Process CPU time excludes pauses caused by other work scheduled on the
/// machine. The parser is single-threaded, so this is the least noisy clock
/// for throughput measurements.
#[cfg(unix)]
struct BenchmarkTimer(Duration);

#[cfg(unix)]
impl BenchmarkTimer {
    fn start() -> Self {
        Self(process_cpu_time())
    }

    fn elapsed(&self) -> Duration {
        process_cpu_time().saturating_sub(self.0)
    }
}

#[cfg(unix)]
fn process_cpu_time() -> Duration {
    let mut time = MaybeUninit::<libc::timespec>::zeroed();
    let result = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, time.as_mut_ptr()) };
    assert_eq!(result, 0, "failed to read process CPU time");
    let time = unsafe { time.assume_init() };
    Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
}

#[cfg(not(unix))]
struct BenchmarkTimer(Instant);

#[cfg(not(unix))]
impl BenchmarkTimer {
    fn start() -> Self {
        Self(Instant::now())
    }

    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

#[cfg(unix)]
fn peak_rss_bytes() -> u64 {
    let mut usage = MaybeUninit::<libc::rusage>::zeroed();
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return 0;
    }

    let raw = unsafe { usage.assume_init() }.ru_maxrss.max(0) as u64;
    if cfg!(any(target_os = "linux", target_os = "android")) {
        raw.saturating_mul(1024)
    } else {
        raw
    }
}

#[cfg(not(unix))]
const fn peak_rss_bytes() -> u64 {
    0
}

fn get_language(path: &Path) -> Language {
    let src_path = GRAMMARS_DIR.join(path).join("src");
    TEST_LOADER
        .load_language_at_path(CompileConfig::new(&src_path, None, None))
        .with_context(|| format!("Failed to load language at path {}", src_path.display()))
        .unwrap()
}
