{
  lib,
  src,
  version,
  rustPlatform,
  stdenv,
}:
# The tree-sitter core now lives in Rust (lib/src_rust). The `tree-sitter` crate
# is rlib-only (like upstream); the standalone C library is produced by asking
# cargo for the `cdylib` + `staticlib` crate-types explicitly, which emits
# libtree_sitter.{a,so,dylib} (Rust core + the lexer logging shim). This
# derivation builds that library and installs it with the public header and
# pkg-config file.
#
# NOTE: this replaced the former CMake-based build; the install layout below
# (esp. the shared-library soname/install_name handling) should be verified on
# a Nix builder.
rustPlatform.buildRustPackage {
  pname = "tree-sitter";
  inherit src version;

  cargoLock.lockFile = ../Cargo.lock;

  # `cargo build` on an rlib-only crate produces no C library, so build the
  # cdylib + staticlib explicitly. cargo reads CARGO_BUILD_TARGET from the
  # environment for cross builds.
  buildPhase = ''
    runHook preBuild
    cargo rustc --release --offline -j $NIX_BUILD_CORES \
      --package tree-sitter --crate-type cdylib --crate-type staticlib
    runHook postBuild
  '';

  doCheck = false;

  # buildRustPackage's default installer only handles binaries, so install the
  # C library artifacts by hand.
  installPhase =
    let
      soext = stdenv.hostPlatform.extensions.sharedLibrary; # ".so" / ".dylib"
      major = lib.versions.major version;
      minor = lib.versions.minor version;
    in
    ''
      runHook preInstall

      mkdir -p $out/lib/pkgconfig $out/include/tree_sitter

      # The release dir differs between native and cross builds; locate it.
      releaseDir=$(dirname "$(find target -name 'libtree_sitter.a' -print -quit)")

      install -m644 lib/include/tree_sitter/api.h $out/include/tree_sitter/api.h
      install -m644 "$releaseDir/libtree_sitter.a" $out/lib/libtree-sitter.a

      sharedSrc="$releaseDir/libtree_sitter${soext}"
      if [ -f "$sharedSrc" ]; then
    ''
    + (
      if stdenv.hostPlatform.isDarwin then
        ''
          install -m755 "$sharedSrc" "$out/lib/libtree-sitter.${major}.${minor}${soext}"
          ln -sf "libtree-sitter.${major}.${minor}${soext}" "$out/lib/libtree-sitter.${major}${soext}"
          ln -sf "libtree-sitter.${major}${soext}" "$out/lib/libtree-sitter${soext}"
        ''
      else
        ''
          install -m755 "$sharedSrc" "$out/lib/libtree-sitter${soext}.${major}.${minor}"
          ln -sf "libtree-sitter${soext}.${major}.${minor}" "$out/lib/libtree-sitter${soext}.${major}"
          ln -sf "libtree-sitter${soext}.${major}" "$out/lib/libtree-sitter${soext}"
        ''
    )
    + ''
      fi

      substitute lib/tree-sitter.pc.in $out/lib/pkgconfig/tree-sitter.pc \
        --replace-fail "@CMAKE_INSTALL_PREFIX@" "$out" \
        --replace-fail "@CMAKE_INSTALL_LIBDIR@" "lib" \
        --replace-fail "@CMAKE_INSTALL_INCLUDEDIR@" "include" \
        --replace-fail "@PROJECT_VERSION@" "${version}" \
        --replace-fail "@PROJECT_DESCRIPTION@" "An incremental parsing system for programming tools" \
        --replace-fail "@PROJECT_HOMEPAGE_URL@" "https://tree-sitter.github.io/tree-sitter/"

      runHook postInstall
    '';

  meta = {
    description = "Tree-sitter incremental parsing library";
    longDescription = ''
      Tree-sitter is a parser generator tool and an incremental parsing library.
      It can build a concrete syntax tree for a source file and efficiently update
      the syntax tree as the source file is edited. This package provides the core
      library (Rust core with a C ABI) that can be used to parse source code using
      Tree-sitter grammars.
    '';
    homepage = "https://tree-sitter.github.io/tree-sitter";
    changelog = "https://github.com/tree-sitter/tree-sitter/releases/tag/v${version}";
    license = lib.licenses.mit;
    maintainers = [ lib.maintainers.amaanq ];
    platforms = lib.platforms.all;
  };
}
