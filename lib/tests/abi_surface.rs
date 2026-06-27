//! Freezes the C-ABI surface exported by the Rust core in `lib/src_rust`.
//!
//! Every `#[no_mangle]` function in `src_rust` is a symbol that the still-C code
//! (`query.c`, `wasm_store.c`) and external consumers link against through
//! `tree_sitter/api.h`. Because C has no name mangling, a silently-changed
//! signature still links and then corrupts at runtime — the linker will not
//! catch it. This test extracts every export's normalized signature and compares
//! it to a committed golden snapshot, so any accidental rename, argument change,
//! or return-type change during refactoring fails fast.
//!
//! The snapshot is whitespace-normalized, so reformatting (rustfmt) does not
//! cause spurious diffs — only a real name/argument/return-type change does.
//!
//! To intentionally change the ABI (rare — should be a deliberate, reviewed act),
//! regenerate the snapshot:
//!
//! ```sh
//! UPDATE_ABI_GOLDEN=1 cargo test -p tree-sitter --test abi_surface
//! ```

use std::fs;
use std::path::{Path, PathBuf};

fn src_rust_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src_rust")
}

fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/abi_surface.golden")
}

/// Collapse all runs of ASCII whitespace to a single space and trim.
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Pull the symbol name out of a `... fn NAME(...)` signature.
fn fn_name(sig: &str) -> Option<&str> {
    let after_fn = sig.split(" fn ").nth(1)?;
    let end = after_fn
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(after_fn.len());
    Some(&after_fn[..end])
}

/// Extract `(name, normalized signature)` for every `#[no_mangle]` export in one file.
fn extract_exports(src: &str) -> Vec<(String, String)> {
    const MARKER: &str = "#[no_mangle]";
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = src[search_from..].find(MARKER) {
        let pos = search_from + rel;
        search_from = pos + MARKER.len();
        // Only a line-leading `#[no_mangle]` is an attribute. Mentions inside
        // comments or strings (e.g. a doc comment describing the porting plan)
        // have non-whitespace before them on the line and must be ignored.
        let line_start = src[..pos].rfind('\n').map_or(0, |n| n + 1);
        if !src[line_start..pos].trim().is_empty() {
            continue;
        }
        let rest = &src[search_from..];
        // The function body opens at the first `{`; these extern fns have no
        // generics/where-clauses, so the first brace is unambiguously the body.
        let Some(brace) = rest.find('{') else {
            continue;
        };
        let head = &rest[..brace];
        if !head.contains(" fn ") {
            continue;
        }
        let sig = normalize_ws(head);
        if let Some(name) = fn_name(&sig) {
            out.push((name.to_string(), sig));
        }
    }
    out
}

/// Build the full snapshot: one `name\tsignature` line per export, sorted.
fn build_snapshot() -> String {
    let dir = src_rust_dir();
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    files.sort();

    let mut lines = Vec::new();
    for path in &files {
        let src = fs::read_to_string(path).unwrap();
        for (name, sig) in extract_exports(&src) {
            lines.push(format!("{name}\t{sig}"));
        }
    }
    lines.sort();
    let mut snapshot = lines.join("\n");
    snapshot.push('\n');
    snapshot
}

#[test]
fn abi_surface_is_frozen() {
    let current = build_snapshot();
    let golden = golden_path();

    if std::env::var_os("UPDATE_ABI_GOLDEN").is_some() {
        fs::write(&golden, &current).unwrap();
        eprintln!(
            "updated {} ({} exports)",
            golden.display(),
            current.lines().count()
        );
        return;
    }

    let expected = fs::read_to_string(&golden).unwrap_or_else(|_| {
        panic!(
            "missing {}. Generate it with:\n  \
             UPDATE_ABI_GOLDEN=1 cargo test -p tree-sitter --test abi_surface",
            golden.display()
        )
    });

    if current != expected {
        let cur: Vec<&str> = current.lines().collect();
        let exp: Vec<&str> = expected.lines().collect();
        let added: Vec<&&str> = cur.iter().filter(|l| !exp.contains(l)).collect();
        let removed: Vec<&&str> = exp.iter().filter(|l| !cur.contains(l)).collect();
        panic!(
            "C-ABI surface of lib/src_rust changed.\n\
             {} exports added/changed:\n{}\n\
             {} exports removed/changed:\n{}\n\n\
             If this change is INTENTIONAL, regenerate the snapshot:\n  \
             UPDATE_ABI_GOLDEN=1 cargo test -p tree-sitter --test abi_surface",
            added.len(),
            added
                .iter()
                .map(|l| format!("  + {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
            removed.len(),
            removed
                .iter()
                .map(|l| format!("  - {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
}
