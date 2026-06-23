use std::{env, fs, path::PathBuf};

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap();
    let core_impl = CoreImpl::from_env();

    #[cfg(feature = "bindgen")]
    generate_bindings(&out_dir);

    fs::copy(
        "src/wasm/stdlib-symbols.txt",
        out_dir.join("stdlib-symbols.txt"),
    )
    .unwrap();

    let mut config = cc::Build::new();

    println!("cargo:rerun-if-env-changed=TREE_SITTER_CORE_IMPL");
    println!("cargo:rustc-check-cfg=cfg(tree_sitter_c_core)");
    if core_impl == CoreImpl::C {
        println!("cargo:rustc-cfg=tree_sitter_c_core");
    }

    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_WASM");
    if env::var("CARGO_FEATURE_WASM").is_ok() {
        config
            .define("TREE_SITTER_FEATURE_WASM", "")
            .define("static_assert(...)", "")
            .include(env::var("DEP_WASMTIME_C_API_INCLUDE").unwrap());
    }

    let manifest_path = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let include_path = manifest_path.join("include");
    let src_path = manifest_path.join("src");
    let core_src_path = core_impl.source_path(&src_path);
    let wasm_path = core_src_path.join("wasm");

    if target.starts_with("wasm32-unknown") {
        configure_wasm_build(&mut config);
    }

    for entry in fs::read_dir(&core_src_path).unwrap() {
        let entry = entry.unwrap();
        let path = core_src_path.join(entry.file_name());
        println!("cargo:rerun-if-changed={}", path.to_str().unwrap());
    }

    config
        .flag_if_supported("-std=c11")
        .flag_if_supported("-fvisibility=hidden")
        .flag_if_supported("-Wshadow")
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-incompatible-pointer-types")
        .include(&core_src_path)
        .include(&wasm_path)
        .include(&include_path)
        .define("_POSIX_C_SOURCE", "200112L")
        .define("_DEFAULT_SOURCE", None)
        .define("_BSD_SOURCE", None)
        .define("_DARWIN_C_SOURCE", None)
        .warnings(false)
        .file(core_src_path.join(core_impl.library_source()));

    if core_impl == CoreImpl::Rust {
        config.file(src_path.join("lexer_log_shim.c"));
    }

    config.compile("tree-sitter");

    println!("cargo:include={}", include_path.display());
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CoreImpl {
    Rust,
    C,
}

impl CoreImpl {
    fn from_env() -> Self {
        println!("cargo:rerun-if-env-changed=TREE_SITTER_C_CORE_SRC_DIR");
        match env::var("TREE_SITTER_CORE_IMPL") {
            Ok(value) if matches!(value.as_str(), "c" | "C" | "c-core" | "lib.c") => Self::C,
            Ok(value)
                if matches!(
                    value.as_str(),
                    "rust" | "Rust" | "rust-core" | "remaining_lib.c" | ""
                ) =>
            {
                Self::Rust
            }
            Ok(value) => {
                panic!("TREE_SITTER_CORE_IMPL must be 'rust' or 'c', got '{value}'");
            }
            Err(_) => Self::Rust,
        }
    }

    const fn library_source(self) -> &'static str {
        match self {
            Self::Rust => "remaining_lib.c",
            Self::C => "lib.c",
        }
    }

    fn source_path(self, default_src_path: &std::path::Path) -> PathBuf {
        match self {
            Self::Rust => default_src_path.to_path_buf(),
            Self::C => env::var("TREE_SITTER_C_CORE_SRC_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    panic!(
                        "TREE_SITTER_C_CORE_SRC_DIR must point to a pre-rewrite lib/src directory when TREE_SITTER_CORE_IMPL=c"
                    );
                }),
        }
    }
}

fn configure_wasm_build(config: &mut cc::Build) {
    let Ok(wasm_headers) = env::var("DEP_TREE_SITTER_LANGUAGE_WASM_HEADERS") else {
        panic!("Environment variable DEP_TREE_SITTER_LANGUAGE_WASM_HEADERS must be set by the language crate");
    };
    let Ok(wasm_src) = env::var("DEP_TREE_SITTER_LANGUAGE_WASM_SRC").map(PathBuf::from) else {
        panic!("Environment variable DEP_TREE_SITTER_LANGUAGE_WASM_SRC must be set by the language crate");
    };

    config.include(&wasm_headers);
    config.files([
        wasm_src.join("stdio.c"),
        wasm_src.join("stdlib.c"),
        wasm_src.join("string.c"),
        wasm_src.join("wctype.c"),
    ]);
}

#[cfg(feature = "bindgen")]
fn generate_bindings(out_dir: &std::path::Path) {
    use std::str::FromStr;

    use bindgen::RustTarget;

    const HEADER_PATH: &str = "include/tree_sitter/api.h";

    println!("cargo:rerun-if-changed={HEADER_PATH}");

    let no_copy = [
        "TSInput",
        "TSLanguage",
        "TSLogger",
        "TSLookaheadIterator",
        "TSParser",
        "TSTree",
        "TSQuery",
        "TSQueryCursor",
        "TSQueryCapture",
        "TSQueryMatch",
        "TSQueryPredicateStep",
    ];

    let rust_version = env!("CARGO_PKG_RUST_VERSION");

    let bindings = bindgen::Builder::default()
        .header(HEADER_PATH)
        .layout_tests(false)
        .allowlist_type("^TS.*")
        .allowlist_function("^ts_.*")
        .allowlist_var("^TREE_SITTER.*")
        .no_copy(no_copy.join("|"))
        .prepend_enum_name(false)
        .use_core()
        .clang_arg("-D TREE_SITTER_FEATURE_WASM")
        .rust_target(RustTarget::from_str(rust_version).unwrap())
        .generate()
        .expect("Failed to generate bindings");

    let bindings_rs = out_dir.join("bindings.rs");
    bindings.write_to_file(&bindings_rs).unwrap_or_else(|_| {
        panic!(
            "Failed to write bindings into path: {}",
            bindings_rs.display()
        )
    });
}
