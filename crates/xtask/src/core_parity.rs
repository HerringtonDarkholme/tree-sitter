use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{bail, Context, Result};

use crate::{root_dir, CoreParity};

const LIBRARY_PROBE_SOURCE: &str = r#"
use std::{
    ffi::CStr,
    os::raw::{c_char, c_void},
    ptr,
};
use tree_sitter::{InputEdit, Parser, Point, Range};

extern "C" {
    fn free(ptr: *mut c_void);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let language: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    println!("language.const.abi={}", tree_sitter::LANGUAGE_VERSION);
    println!(
        "language.const.min_abi={}",
        tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION
    );
    println!("language.abi={}", language.abi_version());
    println!("language.node_kind_count={}", language.node_kind_count());
    println!("language.field_count={}", language.field_count());
    let identifier_kind = language.id_for_node_kind("identifier", true);
    println!(
        "language.kind.identifier={}",
        identifier_kind
    );
    println!(
        "language.kind.identifier.back={}",
        language
            .node_kind_for_id(identifier_kind)
            .unwrap_or("<missing>")
    );
    let name_field = language.field_id_for_name("name").expect("name field");
    println!("language.field.name={}", name_field.get());
    println!(
        "language.field.name.back={}",
        language.field_name_for_id(name_field.get()).unwrap_or("<missing>")
    );

    let source = "let icon = '🦀';\nclass A { method(x: number) { return x + 1; } }\n";
    let mut parser = Parser::new();
    parser.set_language(&language)?;
    let mut tree = parser.parse(source, None).expect("parse tree");

    {
        let root = tree.root_node();
        emit_node("root", root);
        println!("root.has_error={}", root.has_error());
        println!("root.id.stable={}", root.id() == root.id());
        println!("root.sexp={}", root.to_sexp());

        let first_child = root.child(0).expect("first child");
        emit_node("first_child", first_child);
        println!(
            "first_child.text={}",
            first_child.utf8_text(source.as_bytes())?
        );

        let class_node = root.named_child(1).expect("class declaration");
        emit_node("class", class_node);
        println!("class.eq.self={}", class_node == class_node);
        println!("class.eq.root={}", class_node == root);
        println!("class.parse_state={}", class_node.parse_state());
        println!("class.next_parse_state={}", class_node.next_parse_state());
        println!("class.grammar_id={}", class_node.grammar_id());
        println!(
            "root.child_with_class={}",
            root.child_with_descendant(class_node)
                .expect("child containing class")
                .kind()
        );
        emit_node(
            "class.name",
            class_node
                .child_by_field_name("name")
                .expect("class name field"),
        );
        println!(
            "class.name.by_id={}",
            class_node
                .child_by_field_id(name_field.get())
                .expect("class name field by id")
                .kind()
        );
        println!("class.parent={}", class_node.parent().unwrap().kind());

        let mut cursor = root.walk();
        println!("cursor.0={}", cursor.node().kind());
        println!("cursor.0.depth={}", cursor.depth());
        println!("cursor.0.descendant_index={}", cursor.descendant_index());
        println!("cursor.first={}", cursor.goto_first_child());
        println!("cursor.1={}", cursor.node().kind());
        println!("cursor.1.field={}", cursor.field_name().unwrap_or("<none>"));
        println!("cursor.1.depth={}", cursor.depth());
        println!("cursor.1.descendant_index={}", cursor.descendant_index());
        println!(
            "cursor.1.field_id={}",
            cursor
                .field_id()
                .map(|id| id.get().to_string())
                .unwrap_or_else(|| "<none>".into())
        );
        println!("cursor.next={}", cursor.goto_next_sibling());
        println!("cursor.2={}", cursor.node().kind());
        println!("cursor.prev={}", cursor.goto_previous_sibling());
        println!("cursor.3={}", cursor.node().kind());

        let mut byte_cursor = root.walk();
        println!(
            "cursor.byte_index={:?}",
            byte_cursor.goto_first_child_for_byte(class_node.start_byte())
        );
        println!("cursor.byte_node={}", byte_cursor.node().kind());
        let mut point_cursor = root.walk();
        println!(
            "cursor.point_index={:?}",
            point_cursor.goto_first_child_for_point(class_node.start_position())
        );
        println!("cursor.point_node={}", point_cursor.node().kind());
        let mut descendant_cursor = root.walk();
        descendant_cursor.goto_descendant(1);
        println!("cursor.descendant.1={}", descendant_cursor.node().kind());
    }

    emit_c_api_probe(&language, source)?;

    let insert = "let value = new A().method(2);\n";
    let start_byte = source.len();
    let old_end_byte = start_byte;
    let new_end_byte = start_byte + insert.len();
    let edit = InputEdit {
        start_byte,
        old_end_byte,
        new_end_byte,
        start_position: point_for_offset(source, start_byte),
        old_end_position: point_for_offset(source, old_end_byte),
        new_end_position: point_for_offset(&format!("{source}{insert}"), new_end_byte),
    };
    tree.edit(&edit);
    let edited_source = format!("{source}{insert}");
    let edited_tree = parser
        .parse(&edited_source, Some(&tree))
        .expect("incremental parse tree");
    println!("old_tree.changed={}", tree.root_node().has_changes());
    println!("edited.has_error={}", edited_tree.root_node().has_error());
    println!("edited.sexp={}", edited_tree.root_node().to_sexp());
    println!(
        "changed.count={}",
        tree.changed_ranges(&edited_tree).collect::<Vec<_>>().len()
    );

    let ranged_source = "skip\nlet inside = 1;\nskip\n";
    let start_byte = ranged_source.find("let").unwrap();
    let end_byte = start_byte + "let inside = 1;\n".len();
    parser.set_included_ranges(&[Range {
        start_byte,
        end_byte,
        start_point: point_for_offset(ranged_source, start_byte),
        end_point: point_for_offset(ranged_source, end_byte),
    }])?;
    let included_tree = parser.parse(ranged_source, None).expect("included parse tree");
    println!("included.sexp={}", included_tree.root_node().to_sexp());
    for (index, range) in parser.included_ranges().iter().enumerate() {
        println!(
            "included.range.{index}={}:{}-{}:{} bytes {}..{}",
            range.start_point.row,
            range.start_point.column,
            range.end_point.row,
            range.end_point.column,
            range.start_byte,
            range.end_byte,
        );
    }
    for (index, range) in included_tree.included_ranges().iter().enumerate() {
        println!(
            "tree.included.range.{index}={}:{}-{}:{} bytes {}..{}",
            range.start_point.row,
            range.start_point.column,
            range.end_point.row,
            range.end_point.column,
            range.start_byte,
            range.end_byte,
        );
    }

    Ok(())
}

fn emit_c_api_probe(
    language: &tree_sitter::Language,
    source: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let language = language.clone().into_raw();
        let parser = tree_sitter::ffi::ts_parser_new();
        println!("c.parser.created={}", !parser.is_null());
        println!(
            "c.parser.set_language={}",
            tree_sitter::ffi::ts_parser_set_language(parser, language)
        );
        println!(
            "c.parser.language.same={}",
            tree_sitter::ffi::ts_parser_language(parser) == language
        );
        println!(
            "c.language.abi={}",
            tree_sitter::ffi::ts_language_abi_version(language)
        );
        println!(
            "c.language.symbol_count={}",
            tree_sitter::ffi::ts_language_symbol_count(language)
        );
        let name = b"name";
        let name_field = tree_sitter::ffi::ts_language_field_id_for_name(
            language,
            name.as_ptr().cast::<c_char>(),
            name.len() as u32,
        );
        println!("c.language.field.name={name_field}");
        println!(
            "c.language.field.name.back={}",
            cstr(tree_sitter::ffi::ts_language_field_name_for_id(
                language,
                name_field,
            ))
        );

        let tree = tree_sitter::ffi::ts_parser_parse_string(
            parser,
            ptr::null(),
            source.as_ptr().cast::<c_char>(),
            source.len() as u32,
        );
        println!("c.tree.created={}", !tree.is_null());
        let root = tree_sitter::ffi::ts_tree_root_node(tree);
        emit_c_node("c.root", root);
        let root_string = tree_sitter::ffi::ts_node_string(root);
        println!("c.root.sexp={}", cstr(root_string));
        free(root_string.cast::<c_void>());

        let first_child = tree_sitter::ffi::ts_node_child(root, 0);
        emit_c_node("c.first_child", first_child);
        let class_node = tree_sitter::ffi::ts_node_named_child(root, 1);
        emit_c_node("c.class", class_node);
        let class_name = tree_sitter::ffi::ts_node_child_by_field_id(class_node, name_field);
        emit_c_node("c.class.name", class_name);
        println!(
            "c.class.parent={}",
            node_kind(tree_sitter::ffi::ts_node_parent(class_node))
        );
        println!(
            "c.root.child_with_class={}",
            node_kind(tree_sitter::ffi::ts_node_child_with_descendant(
                root, class_node,
            ))
        );
        println!(
            "c.root.descendant_count={}",
            tree_sitter::ffi::ts_node_descendant_count(root)
        );

        let mut cursor = tree_sitter::ffi::ts_tree_cursor_new(root);
        println!(
            "c.cursor.0={}",
            node_kind(tree_sitter::ffi::ts_tree_cursor_current_node(&cursor))
        );
        println!(
            "c.cursor.0.depth={}",
            tree_sitter::ffi::ts_tree_cursor_current_depth(&cursor)
        );
        println!(
            "c.cursor.0.descendant_index={}",
            tree_sitter::ffi::ts_tree_cursor_current_descendant_index(&cursor)
        );
        println!(
            "c.cursor.first={}",
            tree_sitter::ffi::ts_tree_cursor_goto_first_child(&mut cursor)
        );
        println!(
            "c.cursor.1={}",
            node_kind(tree_sitter::ffi::ts_tree_cursor_current_node(&cursor))
        );
        println!(
            "c.cursor.1.field={}",
            cstr(tree_sitter::ffi::ts_tree_cursor_current_field_name(&cursor))
        );
        println!(
            "c.cursor.1.field_id={}",
            tree_sitter::ffi::ts_tree_cursor_current_field_id(&cursor)
        );
        println!(
            "c.cursor.next={}",
            tree_sitter::ffi::ts_tree_cursor_goto_next_sibling(&mut cursor)
        );
        println!(
            "c.cursor.2={}",
            node_kind(tree_sitter::ffi::ts_tree_cursor_current_node(&cursor))
        );
        println!(
            "c.cursor.prev={}",
            tree_sitter::ffi::ts_tree_cursor_goto_previous_sibling(&mut cursor)
        );
        println!(
            "c.cursor.3={}",
            node_kind(tree_sitter::ffi::ts_tree_cursor_current_node(&cursor))
        );
        tree_sitter::ffi::ts_tree_cursor_delete(&mut cursor);

        let mut included_count = 0;
        let included_ranges =
            tree_sitter::ffi::ts_tree_included_ranges(tree, &mut included_count);
        println!("c.tree.included.count={included_count}");
        if !included_ranges.is_null() {
            free(included_ranges.cast::<c_void>());
        }

        let tree_copy = tree_sitter::ffi::ts_tree_copy(tree);
        let insert = "let value = new A().method(2);\n";
        let start_byte = source.len() as u32;
        let new_end_byte = start_byte + insert.len() as u32;
        let edit = tree_sitter::ffi::TSInputEdit {
            start_byte,
            old_end_byte: start_byte,
            new_end_byte,
            start_point: ffi_point_for_offset(source, start_byte as usize),
            old_end_point: ffi_point_for_offset(source, start_byte as usize),
            new_end_point: ffi_point_for_offset(&format!("{source}{insert}"), new_end_byte as usize),
        };
        tree_sitter::ffi::ts_tree_edit(tree_copy, &edit);
        let edited_source = format!("{source}{insert}");
        let edited_tree = tree_sitter::ffi::ts_parser_parse_string(
            parser,
            tree_copy,
            edited_source.as_ptr().cast::<c_char>(),
            edited_source.len() as u32,
        );
        let mut changed_count = 0;
        let changed_ranges = tree_sitter::ffi::ts_tree_get_changed_ranges(
            tree_copy,
            edited_tree,
            &mut changed_count,
        );
        println!("c.changed.count={changed_count}");
        if !changed_ranges.is_null() {
            free(changed_ranges.cast::<c_void>());
        }

        tree_sitter::ffi::ts_tree_delete(edited_tree);
        tree_sitter::ffi::ts_tree_delete(tree_copy);
        tree_sitter::ffi::ts_tree_delete(tree);
        tree_sitter::ffi::ts_parser_delete(parser);
        tree_sitter::ffi::ts_language_delete(language);
    }

    Ok(())
}

fn emit_node(label: &str, node: tree_sitter::Node<'_>) {
    println!("{label}.kind={}", node.kind());
    println!("{label}.kind_id={}", node.kind_id());
    println!("{label}.child_count={}", node.child_count());
    println!("{label}.named_child_count={}", node.named_child_count());
    println!("{label}.start_byte={}", node.start_byte());
    println!("{label}.end_byte={}", node.end_byte());
    println!("{label}.start_point={}", node.start_position());
    println!("{label}.end_point={}", node.end_position());
    println!("{label}.named={}", node.is_named());
    println!("{label}.error={}", node.is_error());
    println!("{label}.missing={}", node.is_missing());
}

fn emit_c_node(label: &str, node: tree_sitter::ffi::TSNode) {
    unsafe {
        println!("{label}.kind={}", node_kind(node));
        println!("{label}.kind_id={}", tree_sitter::ffi::ts_node_symbol(node));
        println!("{label}.child_count={}", tree_sitter::ffi::ts_node_child_count(node));
        println!(
            "{label}.named_child_count={}",
            tree_sitter::ffi::ts_node_named_child_count(node)
        );
        println!("{label}.start_byte={}", tree_sitter::ffi::ts_node_start_byte(node));
        println!("{label}.end_byte={}", tree_sitter::ffi::ts_node_end_byte(node));
        println!(
            "{label}.start_point={}",
            format_point(tree_sitter::ffi::ts_node_start_point(node))
        );
        println!(
            "{label}.end_point={}",
            format_point(tree_sitter::ffi::ts_node_end_point(node))
        );
        println!("{label}.named={}", tree_sitter::ffi::ts_node_is_named(node));
        println!("{label}.error={}", tree_sitter::ffi::ts_node_is_error(node));
        println!("{label}.missing={}", tree_sitter::ffi::ts_node_is_missing(node));
    }
}

fn node_kind(node: tree_sitter::ffi::TSNode) -> String {
    unsafe { cstr(tree_sitter::ffi::ts_node_type(node)) }
}

fn cstr(ptr: *const c_char) -> String {
    if ptr.is_null() {
        "<null>".into()
    } else {
        unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
    }
}

fn format_point(point: tree_sitter::ffi::TSPoint) -> String {
    format!("{}:{}", point.row, point.column)
}

fn ffi_point_for_offset(source: &str, offset: usize) -> tree_sitter::ffi::TSPoint {
    let point = point_for_offset(source, offset);
    tree_sitter::ffi::TSPoint {
        row: point.row as u32,
        column: point.column as u32,
    }
}

fn point_for_offset(source: &str, offset: usize) -> Point {
    let mut row = 0;
    let mut column = 0;
    for byte in &source.as_bytes()[..offset] {
        if *byte == b'\n' {
            row += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    Point::new(row, column)
}
"#;

pub fn run(args: &CoreParity) -> Result<()> {
    let root = root_dir();
    preflight_c_core_revision(root, &args.c_core_rev)?;
    let c_core_src = materialize_c_core(root, &args.c_core_rev)?;
    let default_parent = root
        .parent()
        .context("Failed to find tree-sitter parent directory")?;
    let tree_sitter_typescript = args
        .tree_sitter_typescript_path
        .clone()
        .unwrap_or_else(|| default_parent.join("tree-sitter-typescript"));
    let typescript = args
        .typescript_path
        .clone()
        .unwrap_or_else(|| default_parent.join("typescript"));

    ensure_dir(&tree_sitter_typescript)?;
    ensure_dir(&typescript)?;

    let ts_grammar = tree_sitter_typescript.join("typescript");
    let tsx_grammar = tree_sitter_typescript.join("tsx");
    ensure_dir(&ts_grammar)?;
    ensure_dir(&tsx_grammar)?;
    ensure_file(&ts_grammar.join("src/parser.c"))?;
    ensure_file(&tsx_grammar.join("src/parser.c"))?;
    ensure_dir(&tree_sitter_typescript.join("test/corpus"))?;

    let c_cli = build_cli(root, "c", Some(&c_core_src))?;
    let rust_cli = build_cli(root, "rust", None)?;
    compare_library_probe(root, &tree_sitter_typescript, &c_core_src)?;

    let mut samples = corpus_samples(
        &tree_sitter_typescript,
        &ts_grammar,
        &tsx_grammar,
        args.corpus_sample_limit,
    )?;
    if samples.is_empty() {
        bail!(
            "No tree-sitter-typescript corpus samples found under {}",
            tree_sitter_typescript.join("test/corpus").display(),
        );
    }
    let mut source_samples = typescript_samples(&typescript, &ts_grammar);
    source_samples.truncate(args.sample_limit);
    if source_samples.is_empty() {
        bail!(
            "No focused TypeScript source samples found under {}",
            typescript.display(),
        );
    }
    samples.extend(source_samples);

    let tsx_sample = write_tsx_sample()?;
    samples.push(Sample {
        label: "synthetic TSX smoke".into(),
        language: CorpusLanguage::Tsx,
        grammar_path: tsx_grammar,
        source_path: tsx_sample,
    });

    for sample in &samples {
        println!("checking {}", sample.label);
        compare_sample(sample, &c_cli, &rust_cli)?;
    }
    compare_edit_smoke(&ts_grammar, &c_cli, &rust_cli)?;

    println!("core parity passed for {} samples", samples.len());
    Ok(())
}

fn compare_sample(sample: &Sample, c_cli: &Path, rust_cli: &Path) -> Result<()> {
    compare_parse_output(sample, c_cli, rust_cli, "default", &[])?;
    compare_parse_output(sample, c_cli, rust_cli, "no-ranges", &["--no-ranges"])?;
    Ok(())
}

fn compare_parse_output(
    sample: &Sample,
    c_cli: &Path,
    rust_cli: &Path,
    mode: &str,
    extra_args: &[&str],
) -> Result<()> {
    let c_core = parse_sample(sample, c_cli, extra_args)?;
    let rust_core = parse_sample(sample, rust_cli, extra_args)?;

    if !c_core.status.success() || !rust_core.status.success() {
        bail!(
            "parse command failed for {} ({mode})\n\nC core:\n{}\n\nRust core:\n{}",
            sample.label,
            describe_output(&c_core),
            describe_output(&rust_core),
        );
    }

    if c_core.stdout != rust_core.stdout {
        bail!(
            "parse output differed for {} ({mode})\n\nC core stdout:\n{}\n\nRust core stdout:\n{}",
            sample.label,
            String::from_utf8_lossy(&c_core.stdout),
            String::from_utf8_lossy(&rust_core.stdout),
        );
    }

    Ok(())
}

fn compare_edit_smoke(ts_grammar: &Path, c_cli: &Path, rust_cli: &Path) -> Result<()> {
    let source_path = write_named_sample("tree-sitter-core-parity-edit.ts", "let value = 1;\n")?;
    let sample = Sample {
        label: "incremental edit smoke".into(),
        language: CorpusLanguage::TypeScript,
        grammar_path: ts_grammar.to_path_buf(),
        source_path,
    };
    compare_parse_output(
        &sample,
        c_cli,
        rust_cli,
        "incremental-edit",
        &["--edits", "$ 0 \nlet other = 2;"],
    )
}

fn compare_library_probe(
    root: &Path,
    tree_sitter_typescript: &Path,
    c_core_src: &Path,
) -> Result<()> {
    let probe = write_library_probe(root, tree_sitter_typescript)?;
    let c_core = run_library_probe(&probe, "c", Some(c_core_src))?;
    let rust_core = run_library_probe(&probe, "rust", None)?;

    if !c_core.status.success() || !rust_core.status.success() {
        bail!(
            "library probe failed\n\nC core:\n{}\n\nRust core:\n{}",
            describe_output(&c_core),
            describe_output(&rust_core),
        );
    }

    if c_core.stdout != rust_core.stdout {
        bail!(
            "library probe output differed\n\nC core stdout:\n{}\n\nRust core stdout:\n{}",
            String::from_utf8_lossy(&c_core.stdout),
            String::from_utf8_lossy(&rust_core.stdout),
        );
    }

    Ok(())
}

fn run_library_probe(probe: &Path, core_impl: &str, c_core_src: Option<&Path>) -> Result<Output> {
    let target_dir = std::env::temp_dir()
        .join("tree-sitter-core-parity-probe-target")
        .join(core_impl);
    fs::create_dir_all(&target_dir)
        .with_context(|| format!("Failed to create {}", target_dir.display()))?;

    let mut command = Command::new("cargo");
    command
        .current_dir(probe)
        .env("TREE_SITTER_CORE_IMPL", core_impl)
        .env("CARGO_TARGET_DIR", &target_dir)
        .arg("run")
        .arg("--quiet");

    if let Some(c_core_src) = c_core_src {
        command.env("TREE_SITTER_C_CORE_SRC_DIR", c_core_src);
    }

    command
        .output()
        .with_context(|| format!("Failed to run {command:?}"))
}

fn write_library_probe(root: &Path, tree_sitter_typescript: &Path) -> Result<PathBuf> {
    let probe = std::env::temp_dir().join("tree-sitter-core-parity-probe");
    if probe.exists() {
        fs::remove_dir_all(&probe)
            .with_context(|| format!("Failed to remove {}", probe.display()))?;
    }
    fs::create_dir_all(probe.join("src"))
        .with_context(|| format!("Failed to create {}", probe.join("src").display()))?;
    fs::write(
        probe.join("Cargo.toml"),
        format!(
            r#"[package]
name = "tree-sitter-core-parity-probe"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
tree-sitter = {{ path = "{}" }}
tree-sitter-typescript = {{ path = "{}" }}

[patch.crates-io]
tree-sitter-language = {{ path = "{}" }}
"#,
            toml_path(&root.join("lib")),
            toml_path(tree_sitter_typescript),
            toml_path(&root.join("crates/language")),
        ),
    )
    .with_context(|| format!("Failed to write {}", probe.join("Cargo.toml").display()))?;
    fs::write(probe.join("src").join("main.rs"), LIBRARY_PROBE_SOURCE)
        .with_context(|| format!("Failed to write {}", probe.join("src/main.rs").display()))?;
    Ok(probe)
}

fn toml_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn build_cli(root: &Path, core_impl: &str, c_core_src: Option<&Path>) -> Result<PathBuf> {
    let target_dir = std::env::temp_dir()
        .join("tree-sitter-core-parity-target")
        .join(core_impl);
    fs::create_dir_all(&target_dir)
        .with_context(|| format!("Failed to create {}", target_dir.display()))?;

    let mut command = Command::new("cargo");
    command
        .current_dir(root)
        .env("TREE_SITTER_CORE_IMPL", core_impl)
        .env("CARGO_TARGET_DIR", &target_dir)
        .arg("build")
        .arg("--quiet")
        .arg("-p")
        .arg("tree-sitter-cli")
        .arg("--bin")
        .arg("tree-sitter");

    if let Some(c_core_src) = c_core_src {
        command.env("TREE_SITTER_C_CORE_SRC_DIR", c_core_src);
    }

    let output = command
        .output()
        .with_context(|| format!("Failed to run {command:?}"))?;
    if !output.status.success() {
        bail!(
            "failed to build {core_impl} core CLI\n{}",
            describe_output(&output),
        );
    }

    let binary = target_dir
        .join("debug")
        .join(format!("tree-sitter{}", std::env::consts::EXE_SUFFIX));
    if !binary.is_file() {
        bail!(
            "expected {core_impl} core CLI binary at {}",
            binary.display()
        );
    }

    Ok(binary)
}

fn parse_sample(sample: &Sample, cli: &Path, extra_args: &[&str]) -> Result<Output> {
    let parser_lib_dir = std::env::temp_dir().join("tree-sitter-core-parity-libdir");
    fs::create_dir_all(&parser_lib_dir)
        .with_context(|| format!("Failed to create {}", parser_lib_dir.display()))?;
    let cache_dir = std::env::temp_dir().join("tree-sitter-core-parity-cache");
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("Failed to create {}", cache_dir.display()))?;

    let mut command = Command::new(cli);
    command
        .env("XDG_CACHE_HOME", cache_dir)
        .env("TREE_SITTER_LIBDIR", parser_lib_dir)
        .env("NO_COLOR", "1")
        .arg("parse")
        .arg("--grammar-path")
        .arg(&sample.grammar_path)
        .arg(&sample.source_path)
        .args(extra_args);

    command
        .output()
        .with_context(|| format!("Failed to run {command:?}"))
}

pub(crate) fn materialize_c_core(root: &Path, rev: &str) -> Result<PathBuf> {
    let cache_dir = std::env::temp_dir()
        .join("tree-sitter-core-parity-c-src")
        .join(sanitize_revision(rev));
    let src_dir = cache_dir.join("lib").join("src");
    let complete_marker = cache_dir.join(".complete");
    if complete_marker.is_file() && src_dir.join("lib.c").is_file() {
        return Ok(src_dir);
    }

    if src_dir.exists() {
        fs::remove_dir_all(&src_dir)
            .with_context(|| format!("Failed to remove {}", src_dir.display()))?;
    }
    fs::create_dir_all(&src_dir)
        .with_context(|| format!("Failed to create {}", src_dir.display()))?;

    let files = git_output(root, ["ls-tree", "-r", "--name-only", rev, "lib/src"])?;
    let files = String::from_utf8(files)?;
    for path in files.lines() {
        let relative_path = path
            .strip_prefix("lib/src/")
            .with_context(|| format!("Unexpected lib/src path from git: {path}"))?;
        let destination = src_dir.join(relative_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        let contents = git_output(root, ["show", &format!("{rev}:{path}")])?;
        fs::write(&destination, contents)
            .with_context(|| format!("Failed to write {}", destination.display()))?;
    }

    if !src_dir.join("lib.c").is_file() {
        bail!("Revision {rev} did not provide lib/src/lib.c");
    }

    fs::write(&complete_marker, rev)
        .with_context(|| format!("Failed to write {}", complete_marker.display()))?;

    Ok(src_dir)
}

pub(crate) fn preflight_c_core_revision(root: &Path, rev: &str) -> Result<()> {
    let lib_c = git_output(root, ["show", &format!("{rev}:lib/src/lib.c")])?;
    let lib_c = String::from_utf8(lib_c)?;
    for required_include in [
        "./alloc.c",
        "./get_changed_ranges.c",
        "./language.c",
        "./lexer.c",
        "./node.c",
        "./parser.c",
        "./point.c",
        "./stack.c",
        "./subtree.c",
        "./tree_cursor.c",
        "./tree.c",
    ] {
        if !lib_c.contains(required_include) {
            bail!("C core revision {rev} is missing {required_include} in lib/src/lib.c");
        }
    }
    Ok(())
}

fn git_output<const N: usize>(root: &Path, args: [&str; N]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .context("Failed to run git")?;

    if !output.status.success() {
        bail!(
            "git command failed with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    Ok(output.stdout)
}

fn sanitize_revision(rev: &str) -> String {
    rev.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn typescript_samples(typescript: &Path, grammar_path: &Path) -> Vec<Sample> {
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
    .filter_map(|relative_path| {
        let source_path = typescript.join(relative_path);
        source_path.exists().then(|| Sample {
            label: relative_path.into(),
            language: CorpusLanguage::TypeScript,
            grammar_path: grammar_path.to_path_buf(),
            source_path,
        })
    })
    .collect()
}

fn corpus_samples(
    root: &Path,
    ts_grammar_path: &Path,
    tsx_grammar_path: &Path,
    limit: usize,
) -> Result<Vec<Sample>> {
    let corpus_dir = root.join("test").join("corpus");
    let mut samples = Vec::new();
    let mut corpus_files = fs::read_dir(&corpus_dir)
        .with_context(|| format!("Failed to read {}", corpus_dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    corpus_files.sort();

    for path in corpus_files
        .into_iter()
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("txt"))
    {
        append_corpus_samples(&path, ts_grammar_path, tsx_grammar_path, &mut samples)?;
    }
    Ok(select_corpus_samples(samples, limit))
}

fn append_corpus_samples(
    path: &Path,
    ts_grammar_path: &Path,
    tsx_grammar_path: &Path,
    samples: &mut Vec<Sample>,
) -> Result<()> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("Failed to read corpus file {}", path.display()))?;
    let lines = source.lines().collect::<Vec<_>>();
    let mut i = 0;

    while i + 2 < lines.len() {
        if !is_divider(lines[i]) {
            i += 1;
            continue;
        }

        let Some(header_end) = lines[i + 1..]
            .iter()
            .position(|line| is_divider(line))
            .map(|offset| i + 1 + offset)
        else {
            break;
        };
        let header = &lines[i + 1..header_end];
        let name = header
            .iter()
            .map(|line| line.trim())
            .find(|line| !line.is_empty() && !line.starts_with(':'))
            .unwrap_or("unnamed corpus sample");
        let language = corpus_language(header)?;
        let grammar_path = match language {
            CorpusLanguage::TypeScript => ts_grammar_path,
            CorpusLanguage::Tsx => tsx_grammar_path,
        };

        let mut source_start = header_end + 1;
        if source_start < lines.len() && lines[source_start].is_empty() {
            source_start += 1;
        }

        let Some(source_end) = lines[source_start..]
            .iter()
            .position(|line| line.trim() == "---")
            .map(|offset| source_start + offset)
        else {
            break;
        };

        let snippet = lines[source_start..source_end].join("\n");
        let source_path = write_corpus_sample(path, name, language, &snippet)?;
        samples.push(Sample {
            label: format!(
                "{} - {name} ({})",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("corpus"),
                language.label(),
            ),
            language,
            grammar_path: grammar_path.to_path_buf(),
            source_path,
        });

        i = source_end + 1;
    }

    Ok(())
}

fn select_corpus_samples(mut samples: Vec<Sample>, limit: usize) -> Vec<Sample> {
    if samples.len() <= limit {
        return samples;
    }

    let mut selected = samples.drain(..limit).collect::<Vec<_>>();
    if !selected
        .iter()
        .any(|sample| sample.language == CorpusLanguage::Tsx)
    {
        if let Some(tsx_sample) = samples
            .into_iter()
            .find(|sample| sample.language == CorpusLanguage::Tsx)
        {
            if let Some(last) = selected.last_mut() {
                *last = tsx_sample;
            }
        }
    }
    selected
}

fn corpus_language(header: &[&str]) -> Result<CorpusLanguage> {
    let mut language = CorpusLanguage::TypeScript;
    for line in header {
        let line = line.trim();
        let Some(value) = line
            .strip_prefix(":language(")
            .and_then(|value| value.strip_suffix(')'))
        else {
            continue;
        };
        language = match value {
            "typescript" => CorpusLanguage::TypeScript,
            "tsx" => CorpusLanguage::Tsx,
            value => bail!("Unsupported tree-sitter-typescript corpus language '{value}'"),
        };
    }
    Ok(language)
}

fn is_divider(line: &str) -> bool {
    line.len() >= 3 && line.bytes().all(|byte| byte == b'=')
}

fn write_corpus_sample(
    corpus_path: &Path,
    name: &str,
    language: CorpusLanguage,
    source: &str,
) -> Result<PathBuf> {
    let file_stem = corpus_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("corpus");
    let sample_name = sanitize_revision(&format!("{file_stem}-{name}"));
    write_named_sample(
        &format!(
            "tree-sitter-core-parity-corpus/{sample_name}.{}",
            language.extension()
        ),
        source,
    )
}

fn write_named_sample(name: &str, source: &str) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    fs::write(&path, source).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(path)
}

fn write_tsx_sample() -> Result<PathBuf> {
    let path = std::env::temp_dir().join("tree-sitter-core-parity-sample.tsx");
    fs::write(
        &path,
        r#"export function Widget({ name }: { name: string }) {
  return <section data-name={name}><span>{name.toUpperCase()}</span></section>;
}
"#,
    )
    .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(path)
}

fn ensure_dir(path: &Path) -> Result<()> {
    if path.is_dir() {
        Ok(())
    } else {
        bail!("Required directory does not exist: {}", path.display());
    }
}

fn ensure_file(path: &Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        bail!("Required file does not exist: {}", path.display());
    }
}

fn describe_output(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

struct Sample {
    label: String,
    language: CorpusLanguage,
    grammar_path: PathBuf,
    source_path: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CorpusLanguage {
    TypeScript,
    Tsx,
}

impl CorpusLanguage {
    const fn extension(self) -> &'static str {
        match self {
            Self::TypeScript => "ts",
            Self::Tsx => "tsx",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
        }
    }
}
