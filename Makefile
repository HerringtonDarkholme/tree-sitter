# The tree-sitter library is built by cargo: the Rust core in lib/src_rust plus
# the remaining variadic lexer logging shim. The former pure-C library build
# (libtree-sitter.a/.so, install/uninstall, the amalgamation, and the CMake
# build) has been retired because the core no longer lives in C. To build or
# distribute the library, use cargo (see lib/Cargo.toml `crate-type`, which
# emits a staticlib + cdylib) or the Nix `lib` package.
#
# This Makefile now only provides convenience dev targets.

test:
	cargo xtask fetch-fixtures
	cargo xtask generate-fixtures
	cargo xtask test

test-wasm:
	cargo xtask generate-fixtures --wasm
	cargo xtask test-wasm

lint:
	cargo update --workspace --locked --quiet
	cargo check --workspace --all-targets
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

lint-web:
	npm --prefix lib/binding_web ci
	npm --prefix lib/binding_web run lint

format:
	cargo fmt --all

changelog:
	@git-cliff --config .github/cliff.toml --prepend CHANGELOG.md --latest --github-token $(shell gh auth token)

.PHONY: test test-wasm lint lint-web format changelog
