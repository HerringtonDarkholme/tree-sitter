# Parser performance fixtures

These files are the complete, versioned input corpus for `cargo xtask
perf-gate`. The harness intentionally does not discover source files from
ignored grammar checkouts, environment variables, or sibling repositories.
Every machine therefore parses the same programs.

Small files are retained because they exercise distinct parser shapes. The
harness repeats each file until one timed repetition covers at least 128 KiB,
so keeping a focused fixture does not make timer resolution dominate it.

The Go, C++, and Java sets deliberately cover different loads:

| Language | Fixture group | Primary load |
| --- | --- | --- |
| Go | `proc.go`, `value.go` | large production files, deep declarations and reductions |
| Go | `letter_test.go`, `no_newline_at_eof.go` | literals/tables and EOF behavior |
| Go | `generic_pipeline.go` | generics, interfaces, goroutines, channels, and type switches |
| C++ | `rule.cc`, `marker-index.h` | implementation statements and class-heavy headers |
| C++ | `modern_templates.cpp` | concepts, templates, lambdas, variants, and `requires` |
| C++ | `preprocessor_and_declarations.h` | macros, conditional compilation, attributes, and ABI declarations |
| Java | `Service.java`, `LargeService.java` | compact and ordinary object-oriented code |
| Java | `ModernFeatures.java` | sealed types, records, switch expressions, and lambdas |
| Java | `GenericRepository.java` | annotations, nested generics, streams, and exception paths |

The remaining directories contain the same representative JavaScript,
Python, Rust, and TypeScript inputs that were previously borrowed from grammar
fixtures or a neighboring TypeScript checkout. They are snapshots: update
them only as an explicit benchmark-corpus change and rewrite the checked-in
performance baseline in a separate commit.
