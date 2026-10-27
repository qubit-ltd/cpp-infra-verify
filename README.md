# cpp-infra-verify

[![CI](https://github.com/qubit-ltd/cpp-infra-verify/actions/workflows/ci.yml/badge.svg)](https://github.com/qubit-ltd/cpp-infra-verify/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/qubit-cpp-infra-verify.svg)](https://crates.io/crates/qubit-cpp-infra-verify)
[![Docs](https://img.shields.io/docsrs/qubit-cpp-infra-verify)](https://docs.rs/qubit-cpp-infra-verify)
[![Rust](https://img.shields.io/badge/rust-1.94%2B-orange)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![中文](https://img.shields.io/badge/README-中文-red)](README.zh_CN.md)

A C++ library can build locally yet ship broken headers or unusable CMake exports. `cpp-infra-verify` gives maintainers a real CMake build, CTest results, independent header compilation, and a consumer linked against a relocated installation. Analysis and coverage tools reuse its verified File API build context instead of configuring the project independently.

This repository contains development code, version 0.1.0. It has not been published to crates.io or docs.rs; those badges are project links, not release claims. Linux GCC/Clang with Ninja is exercised by the included fixtures. Other platforms are not yet validated.

## Installation

Build with Rust 1.94 or newer:

```sh
cargo install --path . --locked
cpp-infra-verify --version --json
```

CMake 3.25+ is required; the first release accepts committed Presets schema 6. Tests use CMake 3.28, Ninja, GCC, and `/usr/bin/clang++-18`. The `sanitize` and `fuzz` suites require Linux Clang and Ninja. Conan is needed only when a project profile declares `conan_context`.

## Quick start

For an existing CMake application with `src`, `include`, and `tests` directories, commit `.infra/project.toml`:

```toml
schema = 1
kind = "application"
source_roots = ["src", "include"]
test_roots = ["tests"]

[profiles.debug]
configure = "debug"
build = "debug"
test = "debug"
```

Commit `.infra/verify/config.toml`:

```toml
schema = 1
required_suites = ["build", "test", "headers"]
jobs = 4

[headers]
roots = ["include"]
exclude = []
```

Bind those names in the project's `CMakePresets.json`. `debug` must use an explicit generator and a dedicated `binaryDir` beneath `${sourceDir}/build/infra/`; its build and test presets must select that same configure preset. The project owns its CMake targets and CTest registration. Then run:

```sh
cpp-infra-verify --project . --report build/verify-report.json run --profile debug
cpp-infra-verify --project . --report build/prepare-report.json prepare --profile debug --output build/build-context.json
python3 -c 'import json; r=json.load(open("build/verify-report.json")); assert r["status"]=="passed"'
python3 -c 'import json; c=json.load(open("build/build-context.json")); print(c["binary_dir"], [t["name"] for t in c["targets"]])'
```

The first command executes the selected checks. Explicit `prepare --output` must be a JSON file under the project build directory, outside the CMake binary tree; an existing destination must already be a build context for this project. Sources, configuration, unknown business files and symlink aliases are rejected before building and checked again before writing. The second command really configures and builds, then exports a schema-1 context with the actual compiler, generator, targets, artifacts, optional compilation database, and source/configuration fingerprints. Test selection with zero runnable tests, all skipped tests, and real failures returns a failing exit code. `plan --profile debug` emits a local command graph without invoking external tools.

## Suites and configuration

| Suite | Behavior and required configuration |
| --- | --- |
| `build` | Configure/build the committed profile; discover targets through CMake File API. |
| `test` | Discover and execute the same CTest preset; retain JUnit and reject zero/all-skipped results. |
| `headers` | One translation unit per public header, compiled with declared target usage requirements using the original preset/environment and effective cache in a private build; optional `targets` selects project target names. Exact `exclude` paths require matching `exceptions = { "include/private.ipp" = "include/public.hpp" }` host mappings. |
| `install` | Install to prefix-A, copy to prefix-B, remove the owned prefix-A, and compile/link/run a consumer using the relocated package. Reject exported source/build/original-prefix paths. Applications declare installed executables and smoke arguments. |
| `package` | Execute a CPack preset; extract ZIP/TGZ with traversal, duplicate, link and size checks; verify required files. |
| `sanitize` | Verify preset-provided compile/link instrumentation and execute tests with address+undefined or separate thread sanitizers. |
| `fuzz` | Validate Linux Clang libFuzzer targets; build-only or run bounded smoke jobs against copied corpora; preserve crash artifacts and reproduction arguments. |

`run --suite install` also builds its prerequisites. A single invocation configures and builds once. Explicitly requested but unconfigured suites return configuration errors. Missing optional configuration does not silently enable advanced suites. A failed prerequisite blocks remaining checks.

Library and header-only projects require installation settings; header-only projects still need a buildable compilation fixture and tests. A typical library configuration is:

```toml
[install]
package = "Example"
targets = ["Example::Example"]
headers = ["example/example.hpp"]
consumer_source = "tests/infra/consumer.cpp"
run_consumer = true

[package]
preset = "package"
required_files = ["include/example/example.hpp"]

[sanitize]
sanitizers = ["address", "undefined"]

[fuzz]
mode = "smoke"

[[fuzz.targets]]
target = "example_fuzz"
seconds = 10
max_len = 4096
corpus = "tests/corpus"
timeout_seconds = 5
```

Installed `headers` accept names relative to the standard `include` directory or explicit prefix-relative paths. For applications use `[install] executables = ["bin/example"]`, `smoke_args = []`, and `run_consumer = true`. Cross-compiled consumer execution requires `emulator = ["qemu-..."]`; otherwise explicitly choose `run_consumer = false`, reported as build-only. Configuration/build/test timeout keys default to 300/1800/600 seconds and must be positive. Fuzz `seconds`, `max_len`, and `timeout_seconds` must be positive; the process wall budget is the smoke budget plus per-input timeout and five seconds of shutdown allowance.

The delivered [CMake helpers](conf/cmake/verify.cmake) and [hidden preset bases](conf/presets/verify.json) are listed in [conf/manifest.json](conf/manifest.json). In dedicated profiles call `cpp_infra_sanitizers(target)` or `cpp_infra_fuzzer(target)` after creating your targets. The project must opt into these helpers; verify does not change ordinary compiler flags. Organization-managed policy files and project-owned configuration remain separate.

## Reports and integration

The common report contains tool identity, bundle/run/task IDs, canonical project, source/config digests, status, actual tool versions, command argument arrays, timing, logs, diagnostics, and SHA-256 artifacts. Context files and per-call workspaces live under `build/infra-runs/<run-id>/verify/<invocation-id>/`; artifact paths are relative to the common `report_root`. `--report`, `--offline`, and `--run-context` are supported by every command. Exit codes are 0 success, 1 build/test/gate failure, 2 configuration/protocol error, 3 I/O/spawn error, and 130 cancellation.

The C++ infrastructure entrypoint follows `cpp-infra` main by default. Runtime contexts carry their invocation's immutable tool snapshot; explicit infrastructure updates change project-managed files. A context's `runtime_snapshot` and `runtime_snapshot_digest`, when present, are verified against the snapshot bundle and this executable's hash and identity. Direct development invocation without a context uses bundle `development`; project source/configuration fingerprints are always checked. `CPP_INFRA_BIN_DIR` fixes the sibling-tool directory: a missing dependency binary fails instead of mixing bundles from PATH.

A profile with `conan_context` calls `cpp-infra-dependency prepare --context NAME --output JSON` before acquiring `binary_dir/.cpp-infra.lock`. It rechecks generated dependency files under that lock and compares actual configured compiler/version/standard/build type with Conan settings. Build consumers reacquire the same lock and verify `.cpp-infra-build-state.json` against the context fingerprint. Failed or interrupted rebuilds invalidate the previous successful state. Existing unmarked nonempty directories are not taken over.

Root-level JSON reports are explicitly tracked by `source_excluded_paths` in runtime contexts, reports, and build contexts, so repeated invocations preserve the same source identity. Only normalized, non-input report files qualify: source/test/configuration paths and existing business JSON cannot be overwritten or excluded. Configuration identity never uses these exclusions.

Project/verify TOML files, the root CMakeLists, and recursive preset includes must be ordinary non-symlink files in tracked project locations; excluded build/target/Git/transaction areas cannot supply configuration. Freshness checks revalidate the input list. Build contexts, File API replies and runtime snapshots remain governed by their artifact protocols.

Build contexts record the actual CMake configuration inputs and cache SHA-256. Project toolchains and includes must be tracked inputs; CMake installation modules and Conan-generated toolchains carry separate ownership evidence. Managed engine files are bound to the exact CMake version and prior generation proof, so edited build-directory configuration cannot be adopted by preparing again. Every suite and prepare/run completion recheck this evidence, the entire successful build state, the management marker and generated dependency files before reporting success. Consumers revalidate an existing context through `prepare` with `run-context.build_context`, outside their build lock, then acquire the same lock and verify hashes while consuming it.

Infrastructure initialization creates `.infra/verify/config.toml` only when missing. Its template enables build and test checks; enable header, installation and packaging checks after filling in the project settings. Infrastructure updates preserve this project-owned configuration.

All TOML files require `schema = 1` and reject unknown fields. Paths cannot escape through `..` or symlinks; configured root categories cannot overlap. Preset include/inheritance cycles and conflicting user preset definitions are rejected; unrelated local presets do not affect the committed profile. Only deterministic `sourceDir` and `presetName` macros are supported for build directories. No source/configuration files are edited during checks, and no archives are published.

## Testing

```sh
cargo +nightly fmt --all -- --check --config-path .infra/style/rustfmt.toml
cargo test --locked
cargo clippy --all-targets -- -D warnings
```

Tests use self-contained CMake fixtures, including real GCC/Clang builds, relocated library/header-only/application consumers, multi-configuration builds, CTest skip/failure cases, ZIP/TGZ packaging, ASan failures, libFuzzer crashes, timeouts, cancellation and stale-context rejection. Isolated protocol mocks exercise malformed CTest/dependency responses and unsupported CMake version handling; their build fixtures still invoke real CMake where applicable. A separate local integration also exercises real Conan preparation and context reuse. Tests do not depend on downstream repositories. Actual TSan runtime execution, MSVC, AppleClang, Windows, and real cross-compilation remain integration validation gaps.

## License

Apache License 2.0; see [LICENSE](LICENSE). Copyright © 2026 Haixing Hu.

## Contributing

Keep changes scoped and include meaningful regression fixtures. Run the checks above before submitting a change.

## Author

Haixing Hu <starfish.hu@gmail.com>, Qubit.
