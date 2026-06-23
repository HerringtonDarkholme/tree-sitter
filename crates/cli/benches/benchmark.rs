use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    str,
    sync::LazyLock,
    time::Instant,
};

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
static REPETITION_COUNT: LazyLock<usize> = LazyLock::new(|| {
    env::var("TREE_SITTER_BENCHMARK_REPETITION_COUNT").map_or(5, |s| s.parse::<usize>().unwrap())
});
static ERROR_CASE_LIMIT: LazyLock<Option<usize>> = LazyLock::new(|| {
    env::var("TREE_SITTER_BENCHMARK_ERROR_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
});
static TYPESCRIPT_PATH: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    env::var("TREE_SITTER_BENCHMARK_TYPESCRIPT_PATH")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            let path = ROOT_DIR.parent().unwrap().join("typescript");
            path.is_dir().then_some(path)
        })
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

    fn process_dir(result: &mut BTreeMap<PathBuf, (Vec<PathBuf>, Vec<PathBuf>)>, dir: &Path) {
        if dir.join("grammar.js").exists() {
            let relative_path = dir.strip_prefix(GRAMMARS_DIR.as_path()).unwrap();
            let (example_paths, query_paths) = result.entry(relative_path.to_owned()).or_default();

            example_paths.extend(collect_local_and_parent_files(dir, "examples"));
            query_paths.extend(collect_local_and_parent_files(dir, "queries"));
            if relative_path == Path::new("typescript/typescript") {
                example_paths.extend(typescript_repo_examples());
            }
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

fn typescript_repo_examples() -> Vec<PathBuf> {
    let Some(typescript_path) = TYPESCRIPT_PATH.as_ref() else {
        return Vec::new();
    };

    [
        "src/compiler/builderStatePublic.ts",
        "src/services/transform.ts",
        "src/compiler/corePublic.ts",
        "src/services/refactorProvider.ts",
        "src/server/types.ts",
        "src/server/packageJsonCache.ts",
        "src/compiler/performanceCore.ts",
        "src/server/utilities.ts",
        "src/services/codeFixProvider.ts",
        "src/compiler/performance.ts",
    ]
    .into_iter()
    .map(|path| typescript_path.join(path))
    .filter(|path| path.is_file())
    .collect()
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

        info!("  Constructing Queries");
        for path in query_paths {
            if let Some(filter) = EXAMPLE_FILTER.as_ref() {
                if !path.to_str().unwrap().contains(filter.as_str()) {
                    continue;
                }
            }

            parse(language_name, "query", path, max_path_length, |source| {
                Query::new(&language, str::from_utf8(source).unwrap())
                    .with_context(|| format!("Query file path: {}", path.display()))
                    .expect("Failed to parse query");
            });
        }

        info!("  Parsing Valid Code:");
        let mut normal_speeds = Vec::new();
        for example_path in example_paths {
            if let Some(filter) = EXAMPLE_FILTER.as_ref() {
                if !example_path.to_str().unwrap().contains(filter.as_str()) {
                    continue;
                }
            }

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

        info!("  Parsing Invalid Code (mismatched languages):");
        let mut error_speeds = Vec::new();
        for (other_language_path, (example_paths, _)) in
            EXAMPLE_AND_QUERY_PATHS_BY_LANGUAGE_DIR.iter()
        {
            if other_language_path != language_path {
                for example_path in example_paths
                    .iter()
                    .take((*ERROR_CASE_LIMIT).unwrap_or(usize::MAX))
                {
                    if let Some(filter) = EXAMPLE_FILTER.as_ref() {
                        if !example_path.to_str().unwrap().contains(filter.as_str()) {
                            continue;
                        }
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
    let time = Instant::now();
    for _ in 0..*REPETITION_COUNT {
        action(&source_code);
    }
    let duration = time.elapsed() / (*REPETITION_COUNT as u32);
    let duration_ns = u64::try_from(duration.as_nanos())
        .unwrap_or(u64::MAX)
        .max(1);
    let speed = (source_code.len() as u64 * 1_000_000) / duration_ns;
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
            "bytes": source_code.len() as u64,
            "duration_ns": duration_ns,
            "speed_bytes_per_ms": speed,
        })
    );
    speed as usize
}

fn get_language(path: &Path) -> Language {
    let src_path = GRAMMARS_DIR.join(path).join("src");
    TEST_LOADER
        .load_language_at_path(CompileConfig::new(&src_path, None, None))
        .with_context(|| format!("Failed to load language at path {}", src_path.display()))
        .unwrap()
}
