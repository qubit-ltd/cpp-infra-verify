// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::fs;
use std::path::Path;
use std::process::Command;
use std::process::Output;

use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;

fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().expect("parent")).expect("create directories");
    fs::write(path, text).expect("write fixture");
}
fn fixture(test_body: &str) -> TempDir {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    write(
        root,
        ".infra/project.toml",
        "schema=1\nkind='application'\nsource_roots=['src','include']\ntest_roots=['tests']\n[profiles.debug]\nconfigure='debug'\nbuild='debug'\ntest='debug'\n",
    );
    write(
        root,
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['build','test']\n[headers]\nroots=['include']\nexclude=[]\n",
    );
    write(root,"CMakePresets.json", &json!({"version":6,"configurePresets":[{"name":"debug","generator":"Ninja","binaryDir":"${sourceDir}/build/infra/debug","cacheVariables":{"CMAKE_BUILD_TYPE":"Debug","CMAKE_EXPORT_COMPILE_COMMANDS":"ON"}}],"buildPresets":[{"name":"debug","configurePreset":"debug"}],"testPresets":[{"name":"debug","configurePreset":"debug"}]}).to_string());
    write(
        root,
        "CMakeLists.txt",
        &format!(
            "cmake_minimum_required(VERSION 3.25)\nproject(Fixture LANGUAGES CXX)\nadd_executable(example src/main.cpp)\ntarget_include_directories(example PUBLIC include)\nenable_testing()\n{test_body}\ninstall(TARGETS example RUNTIME DESTINATION bin)\n"
        ),
    );
    write(root, "src/main.cpp", "int main() { return 0; }\n");
    write(
        root,
        "include/example.hpp",
        "#pragma once\ninline int answer() { return 42; }\n",
    );
    write(root, "tests/README", "fixture tests\n");
    temp
}
fn invoke(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"))
        .args([
            "--project",
            root.to_str().expect("path"),
            "--report",
            root.join("build/report.json").to_str().expect("path"),
        ])
        .args(args)
        .output()
        .expect("run CLI")
}
fn assert_exit(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn test_prepare_builds_and_exports_file_api_context() {
    let temp = fixture("add_test(NAME example COMMAND example)");
    let context = temp.path().join("build/context.json");
    assert_exit(
        &invoke(
            temp.path(),
            &[
                "prepare",
                "--profile",
                "debug",
                "--output",
                context.to_str().expect("path"),
            ],
        ),
        0,
    );
    let context: Value = serde_json::from_slice(&fs::read(context).expect("context")).expect("json");
    for key in [
        "schema",
        "project",
        "profile",
        "binary_dir",
        "configure_preset",
        "build_preset",
        "test_preset",
        "configuration",
        "source_digest",
        "config_digest",
        "fingerprint",
        "compiler",
        "generator",
        "compile_commands",
        "targets",
    ] {
        assert!(
            !context[key].is_null() || ["configuration", "test_preset", "compile_commands"].contains(&key),
            "missing {key}"
        );
    }
    assert!(
        context["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .any(|target| target["name"] == "example"
                && target["artifacts"][0].as_str().is_some_and(|p| Path::new(p).is_file()))
    );
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("json");
    assert_eq!(report["status"], "passed");
    assert!(report["commands"].as_array().expect("commands").len() >= 2);
    assert_eq!(report["artifacts"][0]["kind"], "build-context");
}
#[test]
fn test_zero_tests_and_all_disabled_fail_gate() {
    for body in [
        "",
        "add_test(NAME example COMMAND example)\nset_tests_properties(example PROPERTIES DISABLED TRUE)",
    ] {
        let temp = fixture(body);
        assert_exit(
            &invoke(temp.path(), &["run", "--profile", "debug", "--suite", "test"]),
            1,
        );
    }
}
#[test]
fn test_real_ctest_failure_is_preserved() {
    let temp = fixture("add_test(NAME failure COMMAND ${CMAKE_COMMAND} -E false)");
    assert_exit(
        &invoke(temp.path(), &["run", "--profile", "debug", "--suite", "test"]),
        1,
    );
}
#[test]
fn test_unknown_config_and_escape_are_rejected() {
    let temp = fixture("");
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['build']\njobz=3\n",
    );
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['build']\n",
    );
    let presets = fs::read_to_string(temp.path().join("CMakePresets.json"))
        .expect("presets")
        .replace("${sourceDir}/build/infra/debug", "${sourceDir}/../escaped");
    write(temp.path(), "CMakePresets.json", &presets);
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
}
#[test]
fn test_headers_compile_independently_and_reject_missing_include() {
    let temp = fixture("");
    assert_exit(
        &invoke(temp.path(), &["run", "--profile", "debug", "--suite", "headers"]),
        0,
    );
    write(
        temp.path(),
        "include/example.hpp",
        "#pragma once\ninline std::string bad();\n",
    );
    assert_exit(
        &invoke(temp.path(), &["run", "--profile", "debug", "--suite", "headers"]),
        1,
    );
}
#[test]
fn test_application_install_moves_prefix_and_runs_binary() {
    let temp = fixture("");
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['install']\n[install]\nexecutables=['bin/example']\nsmoke_args=[]\nrun_consumer=true\n",
    );
    assert_exit(&invoke(temp.path(), &["run", "--profile", "debug"]), 0);
}
#[test]
fn test_plan_never_invokes_cmake_or_builds() {
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["plan", "--profile", "debug"]), 0);
    assert!(!temp.path().join("build/infra/debug/CMakeCache.txt").exists());
}
#[test]
fn test_source_change_rejects_stale_run_context() {
    let temp = fixture("");
    let run = temp.path().join("build/run-context.json");
    write(temp.path(),"build/run-context.json",&json!({"schema":1,"run_id":"test","task_id":"verify","bundle_id":"development","project":temp.path(),"profile":"debug","config_digest":"stale","report_root":temp.path().join("build/infra-runs/test")}).to_string());
    assert_exit(
        &invoke(
            temp.path(),
            &[
                "--run-context",
                run.to_str().expect("path"),
                "prepare",
                "--profile",
                "debug",
            ],
        ),
        2,
    );
}
#[test]
fn test_version_protocol_json() {
    let out = Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"))
        .args(["--version", "--json"])
        .output()
        .expect("CLI");
    assert_exit(&out, 0);
    let version: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(version["protocol"], 1);
    assert_eq!(version["name"], "cpp-infra-verify");
}

fn library_fixture(header_only: bool) -> TempDir {
    let temp = fixture("");
    write(
        temp.path(),
        ".infra/project.toml",
        &format!(
            "schema=1\nkind='{}'\nsource_roots=['src','include']\ntest_roots=['tests']\n[profiles.debug]\nconfigure='debug'\nbuild='debug'\ntest='debug'\n",
            if header_only { "header-only" } else { "library" }
        ),
    );
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['build','test','headers','install','package']\n[headers]\nroots=['include']\ntargets=['example']\n[install]\npackage='Example'\ntargets=['Example::example']\nheaders=['example.hpp']\nconsumer_source='tests/consumer.cpp'\nrun_consumer=true\n[package]\npreset='package'\nrequired_files=['include/example.hpp']\n",
    );
    write(
        temp.path(),
        "tests/consumer.cpp",
        "#include <example.hpp>\nint main() { return answer()==42 ? 0 : 1; }\n",
    );
    let library = if header_only {
        "add_library(example INTERFACE)\ntarget_include_directories(example INTERFACE $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include> $<INSTALL_INTERFACE:include>)"
    } else {
        "add_library(example STATIC src/library.cpp)\ntarget_include_directories(example PUBLIC $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include> $<INSTALL_INTERFACE:include>)"
    };
    write(
        temp.path(),
        "src/library.cpp",
        "#include <example.hpp>\nint library_answer() { return answer(); }\n",
    );
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!(
            "cmake_minimum_required(VERSION 3.25)\nproject(Example VERSION 1.0 LANGUAGES CXX)\n{library}\nadd_executable(example_test tests/consumer.cpp)\ntarget_link_libraries(example_test PRIVATE example)\nenable_testing()\nadd_test(NAME example_test COMMAND example_test)\ninstall(TARGETS example EXPORT ExampleTargets)\ninstall(DIRECTORY include/ DESTINATION include)\ninstall(EXPORT ExampleTargets NAMESPACE Example:: DESTINATION lib/cmake/Example)\nfile(WRITE ${{CMAKE_CURRENT_BINARY_DIR}}/ExampleConfig.cmake \"include(\\\"\\${{CMAKE_CURRENT_LIST_DIR}}/ExampleTargets.cmake\\\")\\n\")\ninstall(FILES ${{CMAKE_CURRENT_BINARY_DIR}}/ExampleConfig.cmake DESTINATION lib/cmake/Example)\nset(CPACK_GENERATOR ZIP TGZ)\ninclude(CPack)\n"
        ),
    );
    let mut presets: Value =
        serde_json::from_str(&fs::read_to_string(temp.path().join("CMakePresets.json")).expect("presets"))
            .expect("json");
    presets["packagePresets"] = json!([{"name":"package","configurePreset":"debug","generators":["ZIP","TGZ"]}]);
    write(temp.path(), "CMakePresets.json", &presets.to_string());
    temp
}
#[test]
fn test_library_and_header_only_relocated_consumption_and_archives() {
    for header_only in [false, true] {
        let temp = library_fixture(header_only);
        assert_exit(&invoke(temp.path(), &["run", "--profile", "debug"]), 0);
    }
}
#[test]
fn test_install_missing_header_and_missing_export_fail() {
    let temp = library_fixture(false);
    let cfg = fs::read_to_string(temp.path().join(".infra/verify/config.toml")).expect("config");
    write(
        temp.path(),
        ".infra/verify/config.toml",
        &cfg.replace("headers=['example.hpp']", "headers=['missing.hpp']"),
    );
    assert_exit(
        &invoke(temp.path(), &["run", "--profile", "debug", "--suite", "install"]),
        1,
    );
    write(
        temp.path(),
        ".infra/verify/config.toml",
        &cfg.replace("Example::example", "Example::missing"),
    );
    assert_exit(
        &invoke(temp.path(), &["run", "--profile", "debug", "--suite", "install"]),
        1,
    );
}
#[test]
fn test_all_runtime_skipped_tests_fail_gate() {
    let temp = fixture(
        "add_test(NAME skip COMMAND ${CMAKE_COMMAND} -E echo SKIP)\nset_tests_properties(skip PROPERTIES SKIP_REGULAR_EXPRESSION SKIP)",
    );
    assert_exit(
        &invoke(temp.path(), &["run", "--profile", "debug", "--suite", "test"]),
        1,
    );
}
#[test]
fn test_multi_config_requires_selection_and_builds_release_artifact() {
    let temp = fixture("add_test(NAME example COMMAND example)");
    let presets = fs::read_to_string(temp.path().join("CMakePresets.json"))
        .expect("presets")
        .replace("\"Ninja\"", "\"Ninja Multi-Config\"");
    write(temp.path(), "CMakePresets.json", &presets);
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
    let mut presets: Value = serde_json::from_str(&presets).expect("presets");
    presets["buildPresets"][0]["configuration"] = json!("Release");
    presets["testPresets"][0]["configuration"] = json!("Release");
    write(temp.path(), "CMakePresets.json", &presets.to_string());
    assert_exit(&invoke(temp.path(), &["run", "--profile", "debug"]), 0);
    assert!(temp.path().join("build/infra/debug/Release/example").is_file());
}
fn clang_profile(temp: &TempDir, flags: &str) {
    let mut presets: Value =
        serde_json::from_str(&fs::read_to_string(temp.path().join("CMakePresets.json")).expect("presets"))
            .expect("json");
    presets["configurePresets"][0]["cacheVariables"]["CMAKE_CXX_COMPILER"] = json!("/usr/bin/clang++-18");
    presets["configurePresets"][0]["cacheVariables"]["CMAKE_CXX_FLAGS"] = json!(flags);
    write(temp.path(), "CMakePresets.json", &presets.to_string());
}
#[test]
fn test_sanitizer_actual_failure_and_incompatible_selection() {
    let temp = fixture("add_test(NAME example COMMAND example)");
    clang_profile(&temp, "-fsanitize=address,undefined -fno-omit-frame-pointer");
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['sanitize']\n[sanitize]\nsanitizers=['address','undefined']\n",
    );
    assert_exit(&invoke(temp.path(), &["run", "--profile", "debug"]), 0);
    write(
        temp.path(),
        "src/main.cpp",
        "int main(int argc,char**) { int* a=new int[1]; a[argc+2]=9; return a[0]; }\n",
    );
    assert_exit(&invoke(temp.path(), &["run", "--profile", "debug"]), 1);
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['sanitize']\n[sanitize]\nsanitizers=['address','thread']\n",
    );
    assert_exit(&invoke(temp.path(), &["run", "--profile", "debug"]), 2);
}
#[test]
fn test_fuzzer_real_crash_keeps_original_corpus_untouched() {
    let temp = fixture("");
    clang_profile(&temp, "-fsanitize=fuzzer-no-link,address");
    let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!("{cmake}\ntarget_link_options(example PRIVATE -fsanitize=fuzzer,address)\n"),
    );
    write(
        temp.path(),
        "src/main.cpp",
        "#include <cstddef>\n#include <cstdint>\n#include <cstdlib>\nextern \"C\" int LLVMFuzzerTestOneInput(const uint8_t* data, size_t size) { if(size && data[0]=='X') abort(); return 0; }\n",
    );
    write(temp.path(), "tests/corpus/seed", "X");
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['fuzz']\n[fuzz]\nmode='smoke'\n[[fuzz.targets]]\ntarget='example'\nseconds=1\nmax_len=16\ncorpus='tests/corpus'\ntimeout_seconds=3\n",
    );
    assert_exit(&invoke(temp.path(), &["run", "--profile", "debug"]), 1);
    assert_eq!(
        fs::read_dir(temp.path().join("tests/corpus")).expect("corpus").count(),
        1
    );
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("json");
    assert!(
        report["artifacts"]
            .as_array()
            .expect("artifacts")
            .iter()
            .any(|entry| entry["kind"] == "fuzz-crash")
    );
}
#[test]
fn test_source_edit_invalidates_previously_valid_run_context() {
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("json");
    let context = json!({"schema":1,"run_id":"stale-test","task_id":"verify","bundle_id":"development","project":temp.path(),"profile":"debug","config_digest":report["config_digest"],"source_digest":report["source_digest"],"report_root":temp.path().join("build/infra-runs/stale-test")});
    write(temp.path(), "build/run-context.json", &context.to_string());
    write(temp.path(), "src/main.cpp", "int main() { return 1; }\n");
    assert_exit(
        &invoke(
            temp.path(),
            &[
                "--run-context",
                temp.path().join("build/run-context.json").to_str().expect("path"),
                "prepare",
                "--profile",
                "debug",
            ],
        ),
        2,
    );
}
#[test]
fn test_export_source_path_leak_and_missing_transitive_dependency_fail() {
    let temp = library_fixture(false);
    let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!(
            "{cmake}\nfile(APPEND ${{CMAKE_CURRENT_BINARY_DIR}}/ExampleConfig.cmake \"set_property(TARGET Example::example APPEND PROPERTY INTERFACE_INCLUDE_DIRECTORIES {} )\\n\")\n",
            temp.path().join("include").display()
        ),
    );
    assert_exit(
        &invoke(temp.path(), &["run", "--profile", "debug", "--suite", "install"]),
        1,
    );
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!(
            "{cmake}\nadd_library(Missing::Dependency INTERFACE IMPORTED GLOBAL)\ntarget_link_libraries(example PUBLIC Missing::Dependency)\n"
        ),
    );
    assert_exit(
        &invoke(temp.path(), &["run", "--profile", "debug", "--suite", "install"]),
        1,
    );
}
#[cfg(unix)]
#[test]
fn test_sigint_cancels_command_tree_and_writes_cancelled_report() {
    use std::time::Duration;
    use std::time::Instant;
    let temp = fixture("");
    let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!(
            "{cmake}\nadd_custom_target(slow ALL COMMAND ${{CMAKE_COMMAND}} -E touch ${{CMAKE_BINARY_DIR}}/cancel-started COMMAND ${{CMAKE_COMMAND}} -E sleep 60)\n"
        ),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"))
        .args([
            "--project",
            temp.path().to_str().expect("path"),
            "--report",
            temp.path().join("build/report.json").to_str().expect("path"),
            "prepare",
            "--profile",
            "debug",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("CLI");
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if temp.path().join("build/infra/debug/cancel-started").is_file() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // The CLI must translate SIGINT to a common cancellation report and stop
    // children.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    let status = child.wait().expect("child exit");
    assert_eq!(status.code(), Some(130));
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("cancel report")).expect("json");
    assert_eq!(report["status"], "cancelled");
}
#[test]
fn test_build_timeout_fails_with_command_log() {
    let temp = fixture("");
    let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!("{cmake}\nadd_custom_target(slow ALL COMMAND ${{CMAKE_COMMAND}} -E sleep 10)\n"),
    );
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['build']\nbuild_timeout_seconds=1\n",
    );
    let output = invoke(temp.path(), &["prepare", "--profile", "debug"]);
    assert_exit(&output, 1);
    assert!(String::from_utf8_lossy(&output.stderr).contains("timed out"));
}
#[test]
fn test_runtime_snapshot_hash_and_executable_identity_are_verified() {
    use sha2::Digest;
    use sha2::Sha256;
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("json");
    let run_root = temp.path().join("build/infra-runs/snapshot-test");
    let binary = Path::new(env!("CARGO_BIN_EXE_cpp-infra-verify"))
        .canonicalize()
        .expect("binary");
    let snapshot = json!({"schema":1,"bundle_id":"main-runtime","tools":[{"name":"cpp-infra-verify","version":env!("CARGO_PKG_VERSION"),"revision":null,"protocol":1,"binary":binary,"executable_sha256":hex::encode(Sha256::digest(fs::read(&binary).expect("binary")))}],"offline":false,"stale":false});
    write(
        temp.path(),
        "build/infra-runs/snapshot-test/runtime-snapshot.json",
        &snapshot.to_string(),
    );
    let snapshot_path = run_root.join("runtime-snapshot.json");
    let mut context = json!({"schema":1,"run_id":"snapshot-test","task_id":"verify","bundle_id":"main-runtime","project":temp.path(),"profile":"debug","config_digest":report["config_digest"],"source_digest":report["source_digest"],"report_root":run_root,"runtime_snapshot":snapshot_path,"runtime_snapshot_digest":"wrong"});
    write(temp.path(), "build/run-context.json", &context.to_string());
    let context_path = temp.path().join("build/run-context.json");
    let args = [
        "--run-context",
        context_path.to_str().expect("path"),
        "prepare",
        "--profile",
        "debug",
    ];
    assert_exit(&invoke(temp.path(), &args), 2);
    context["runtime_snapshot_digest"] =
        json!(hex::encode(Sha256::digest(fs::read(&snapshot_path).expect("snapshot"))));
    write(temp.path(), "build/run-context.json", &context.to_string());
    assert_exit(&invoke(temp.path(), &args), 0);
    let copied_binary = temp.path().join("build/copied-verify");
    fs::copy(&binary, &copied_binary).expect("copy same bundle binary");
    let mut copied = snapshot.clone();
    copied["tools"][0]["binary"] = json!(copied_binary);
    fs::write(&snapshot_path, copied.to_string()).expect("snapshot");
    context["runtime_snapshot_digest"] =
        json!(hex::encode(Sha256::digest(fs::read(&snapshot_path).expect("snapshot"))));
    write(temp.path(), "build/run-context.json", &context.to_string());
    assert_exit(&invoke(temp.path(), &args), 0);
}
#[test]
fn test_hidden_inheritance_and_include_use_cmake_precedence() {
    let temp = fixture("add_test(NAME example COMMAND example)");
    write(temp.path(),"presets/base.json",&json!({"version":6,"configurePresets":[{"name":"base","hidden":true,"generator":"Ninja","binaryDir":"${sourceDir}/build/infra/${presetName}","cacheVariables":{"CMAKE_BUILD_TYPE":"Debug"}}],"buildPresets":[{"name":"build-base","hidden":true,"configurePreset":"debug"}],"testPresets":[{"name":"test-base","hidden":true,"configurePreset":"debug"}]}).to_string());
    write(temp.path(),"CMakePresets.json",&json!({"version":6,"include":["presets/base.json"],"configurePresets":[{"name":"debug","inherits":"base"}],"buildPresets":[{"name":"debug","inherits":"build-base"}],"testPresets":[{"name":"debug","inherits":"test-base"}]}).to_string());
    assert_exit(&invoke(temp.path(), &["run", "--profile", "debug"]), 0);
}
#[test]
fn test_run_reuses_verified_context_but_executes_real_ctest() {
    let temp = fixture("add_test(NAME example COMMAND example)");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("json");
    let context = json!({"schema":1,"run_id":report["run_id"],"task_id":"verify-test","bundle_id":"development","project":temp.path(),"profile":"debug","config_digest":report["config_digest"],"source_digest":report["source_digest"],"report_root":report["report_root"],"build_context":report["data"]["build_context"]});
    write(temp.path(), "build/run-context.json", &context.to_string());
    assert_exit(
        &invoke(
            temp.path(),
            &[
                "--run-context",
                temp.path().join("build/run-context.json").to_str().expect("path"),
                "run",
                "--profile",
                "debug",
                "--suite",
                "test",
            ],
        ),
        0,
    );
    let result: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("json");
    let commands = result["commands"].as_array().expect("commands");
    assert!(commands.iter().any(|command| command["program"] == "ctest"));
    assert!(
        !commands.iter().any(|command| command["program"] == "cmake"),
        "reused context must avoid configure/build"
    );
    let state = temp.path().join("build/infra/debug/.cpp-infra-build-state.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&state).expect("state")).expect("json");
    value["fingerprint"] = json!("concurrently-changed");
    fs::write(state, value.to_string()).expect("state");
    assert_exit(
        &invoke(
            temp.path(),
            &[
                "--run-context",
                temp.path().join("build/run-context.json").to_str().expect("path"),
                "run",
                "--profile",
                "debug",
                "--suite",
                "test",
            ],
        ),
        2,
    );
}
#[cfg(unix)]
#[test]
fn test_binary_directory_symlink_cannot_write_to_source_tree() {
    let temp = fixture("");
    fs::create_dir_all(temp.path().join("build/infra")).expect("build root");
    std::os::unix::fs::symlink(temp.path().join("src"), temp.path().join("build/infra/debug")).expect("symlink");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
    assert!(
        !temp.path().join("src/.cpp-infra.lock").exists(),
        "reject symlink alias before writing into source"
    );
}
#[test]
fn test_build_directory_lock_is_exclusive_and_bounded() {
    use fs2::FileExt;
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['build']\nconfigure_timeout_seconds=1\n",
    );
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(temp.path().join("build/infra/debug/.cpp-infra.lock"))
        .expect("lock");
    FileExt::lock_exclusive(&lock).expect("hold owner lock");
    let output = invoke(temp.path(), &["prepare", "--profile", "debug"]);
    assert_exit(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("waiting for build directory lock"));
    FileExt::unlock(&lock).expect("unlock");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
}
#[test]
fn test_failed_rebuild_invalidates_previous_success_state() {
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    write(temp.path(), "src/main.cpp", "int main() { compile_error; }\n");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 1);
    let state: Value = serde_json::from_slice(
        &fs::read(temp.path().join("build/infra/debug/.cpp-infra-build-state.json")).expect("state"),
    )
    .expect("json");
    assert!(state["fingerprint"].is_null());
    assert_eq!(state["status"], "building");
}
#[test]
fn test_cross_compiled_install_explicitly_reports_build_only() {
    let temp = fixture("");
    let mut presets: Value =
        serde_json::from_str(&fs::read_to_string(temp.path().join("CMakePresets.json")).expect("presets"))
            .expect("json");
    presets["configurePresets"][0]["cacheVariables"]["CMAKE_SYSTEM_NAME"] = json!("Generic");
    write(temp.path(), "CMakePresets.json", &presets.to_string());
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['install']\n[install]\nexecutables=['bin/example']\nrun_consumer=false\n",
    );
    assert_exit(&invoke(temp.path(), &["run", "--profile", "debug"]), 0);
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("json");
    assert_eq!(report["data"]["suites"][0]["data"]["mode"], "build-only");
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['install']\n[install]\nexecutables=['bin/example']\nrun_consumer=true\n",
    );
    assert_exit(&invoke(temp.path(), &["run", "--profile", "debug"]), 2);
}
#[test]
fn test_fuzz_build_only_rejects_executable_without_libfuzzer_link_runtime() {
    let temp = fixture("");
    clang_profile(&temp, "-fsanitize=fuzzer-no-link,address");
    write(temp.path(), "tests/corpus/seed", "seed");
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['fuzz']\n[fuzz]\nmode='build-only'\n[[fuzz.targets]]\ntarget='example'\nseconds=1\nmax_len=16\ncorpus='tests/corpus'\ntimeout_seconds=3\n",
    );
    assert_exit(&invoke(temp.path(), &["run", "--profile", "debug"]), 2);
}
#[cfg(unix)]
#[test]
fn test_old_cmake_is_classified_as_configuration_error() {
    use std::os::unix::fs::PermissionsExt;
    let temp = fixture("");
    let tools = tempfile::tempdir().expect("tools");
    write(
        tools.path(),
        "cmake",
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'cmake version 3.24.0'; exit 0; fi\nexit 99\n",
    );
    fs::set_permissions(tools.path().join("cmake"), fs::Permissions::from_mode(0o755)).expect("executable");
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"))
        .args([
            "--project",
            temp.path().to_str().expect("path"),
            "--report",
            temp.path().join("build/report.json").to_str().expect("path"),
            "prepare",
            "--profile",
            "debug",
        ])
        .env(
            "PATH",
            format!("{}:{}", tools.path().display(), std::env::var("PATH").expect("PATH")),
        )
        .output()
        .expect("CLI");
    assert_exit(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("CMake 3.25"));
}
#[test]
fn test_unsupported_package_generator_is_rejected_before_cpack() {
    let temp = library_fixture(false);
    let mut presets: Value =
        serde_json::from_str(&fs::read_to_string(temp.path().join("CMakePresets.json")).expect("presets"))
            .expect("json");
    presets["packagePresets"][0]["generators"] = json!(["DEB"]);
    write(temp.path(), "CMakePresets.json", &presets.to_string());
    assert_exit(
        &invoke(temp.path(), &["run", "--profile", "debug", "--suite", "package"]),
        2,
    );
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("json");
    assert!(
        !report["commands"]
            .as_array()
            .expect("commands")
            .iter()
            .any(|command| command["program"] == "cpack")
    );
}
#[test]
fn test_unrelated_user_presets_do_not_change_committed_profile() {
    let temp = fixture("");
    write(temp.path(),"CMakeUserPresets.json",&json!({"version":6,"configurePresets":[{"name":"local-only","inherits":"debug","cacheVariables":{"CMAKE_BUILD_TYPE":"Release"}}]}).to_string());
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    write(temp.path(),"CMakeUserPresets.json",&json!({"version":6,"configurePresets":[{"name":"debug","inherits":"debug","environment":{"CXX":"different"}}]}).to_string());
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
}
#[test]
fn test_shared_preset_include_is_loaded_once_without_false_cycle() {
    let temp = fixture("");
    write(temp.path(),"presets/base.json",&json!({"version":6,"configurePresets":[{"name":"base","hidden":true,"generator":"Ninja","binaryDir":"${sourceDir}/build/infra/${presetName}"}]}).to_string());
    for name in ["a", "b"] {
        write(temp.path(),&format!("presets/{name}.json"),&json!({"version":6,"include":["base.json"],"configurePresets":[{"name":name,"hidden":true,"inherits":"base"}]}).to_string());
    }
    write(temp.path(),"CMakePresets.json",&json!({"version":6,"include":["presets/a.json","presets/b.json"],"configurePresets":[{"name":"debug","inherits":["a","b"]}],"buildPresets":[{"name":"debug","configurePreset":"debug"}],"testPresets":[{"name":"debug","configurePreset":"debug"}]}).to_string());
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
}
#[test]
fn test_header_only_project_without_compilation_fixture_is_rejected() {
    let temp = library_fixture(true);
    write(
        temp.path(),
        "CMakeLists.txt",
        "cmake_minimum_required(VERSION 3.25)\nproject(Fixture LANGUAGES CXX)\nadd_custom_target(marker ALL COMMAND ${CMAKE_COMMAND} -E echo no-compilation)\n",
    );
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
}
fn invoke_report(root: &Path, report: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"))
        .args([
            "--project",
            root.to_str().expect("path"),
            "--report",
            report.to_str().expect("path"),
        ])
        .args(args)
        .output()
        .expect("CLI")
}
#[test]
fn test_root_report_is_excluded_consistently_across_repeated_prepare() {
    let temp = fixture("");
    let path = temp.path().join("result.json");
    let args = ["prepare", "--profile", "debug"];
    assert_exit(&invoke_report(temp.path(), &path, &args), 0);
    let first: Value = serde_json::from_slice(&fs::read(&path).expect("report")).expect("json");
    assert_exit(&invoke_report(temp.path(), &path, &args), 0);
    let second: Value = serde_json::from_slice(&fs::read(&path).expect("report")).expect("json");
    assert_eq!(first["source_digest"], second["source_digest"]);
    assert_eq!(second["source_excluded_paths"], json!(["result.json"]));
    let context: Value = serde_json::from_slice(
        &fs::read(second["data"]["build_context"].as_str().expect("context path")).expect("context"),
    )
    .expect("json");
    assert_eq!(context["source_excluded_paths"], second["source_excluded_paths"]);
}
#[test]
fn test_reports_cannot_overwrite_source_or_unknown_business_json() {
    let temp = fixture("");
    write(temp.path(), "business.json", "{\"orders\": [42]}");
    for name in [
        "src/result.json",
        "README.json",
        "business.json",
        ".infra/result.json",
        "conan/result.json",
    ] {
        assert_exit(
            &invoke_report(temp.path(), &temp.path().join(name), &["prepare", "--profile", "debug"]),
            2,
        );
    }
    assert_eq!(
        fs::read_to_string(temp.path().join("business.json")).expect("business"),
        "{\"orders\": [42]}"
    );
    assert!(!temp.path().join("src/result.json").exists());
}
#[test]
fn test_excluded_source_context_is_rejected_and_transactions_do_not_change_config_digest() {
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let first: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("json");
    let context = json!({"schema":1,"run_id":first["run_id"],"task_id":"verify","bundle_id":"development","project":temp.path(),"profile":"debug","source_digest":first["source_digest"],"config_digest":first["config_digest"],"report_root":first["report_root"],"source_excluded_paths":["src/main.cpp"]});
    write(temp.path(), "build/run-context.json", &context.to_string());
    assert_exit(
        &invoke(
            temp.path(),
            &[
                "--run-context",
                temp.path().join("build/run-context.json").to_str().expect("path"),
                "prepare",
                "--profile",
                "debug",
            ],
        ),
        2,
    );
    write(temp.path(), ".infra/.transactions/scratch.json", "{\"temporary\":1}");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let second: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("json");
    assert_eq!(first["config_digest"], second["config_digest"]);
    write(temp.path(), "src/build/input.cpp", "int input=2;");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let third: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("json");
    assert_ne!(third["source_digest"], second["source_digest"]);
}
#[test]
fn test_prepare_output_is_validated_before_any_source_or_business_overwrite() {
    let temp = fixture("");
    write(temp.path(), "build/business.json", "{\"orders\":[42]}");
    for name in [
        "src/main.cpp",
        ".infra/project.toml",
        "CMakePresets.json",
        "build/business.json",
    ] {
        let path = temp.path().join(name);
        let before = fs::read(&path).expect("original input");
        let output = invoke(
            temp.path(),
            &[
                "prepare",
                "--profile",
                "debug",
                "--output",
                path.to_str().expect("path"),
            ],
        );
        assert_eq!(
            fs::read(&path).expect("unchanged input"),
            before,
            "must validate before overwriting {name}"
        );
        assert_exit(&output, 2);
    }
    let context = temp.path().join("build/prepared.json");
    for _ in 0..2 {
        assert_exit(
            &invoke(
                temp.path(),
                &[
                    "prepare",
                    "--profile",
                    "debug",
                    "--output",
                    context.to_str().expect("path"),
                ],
            ),
            0,
        );
    }
}
#[cfg(unix)]
#[test]
fn test_relocated_application_cannot_read_original_install_prefix() {
    let temp = fixture("");
    let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!(
            "{cmake}\ninstall(CODE [[\nfile(MAKE_DIRECTORY \"${{CMAKE_INSTALL_PREFIX}}/share\")\nfile(WRITE \"${{CMAKE_INSTALL_PREFIX}}/share/data\" \"payload\")\nfile(WRITE \"${{CMAKE_INSTALL_PREFIX}}/bin/example\" \"#!/bin/sh\\ntest -f '${{CMAKE_INSTALL_PREFIX}}/share/data'\\n\")\nfile(CHMOD \"${{CMAKE_INSTALL_PREFIX}}/bin/example\" PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)\n]])\n"
        ),
    );
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['install']\n[install]\nexecutables=['bin/example']\nrun_consumer=true\n",
    );
    assert_exit(&invoke(temp.path(), &["run", "--profile", "debug"]), 1);
}
#[test]
fn test_headers_preserve_preset_cache_and_environment_target_conditions() {
    let temp = fixture("");
    let mut presets: Value =
        serde_json::from_str(&fs::read_to_string(temp.path().join("CMakePresets.json")).expect("presets"))
            .expect("json");
    presets["configurePresets"][0]["cacheVariables"]["ENABLE_API"] = json!("ON");
    presets["configurePresets"][0]["environment"] = json!({"API_MODE":"enabled"});
    write(temp.path(), "CMakePresets.json", &presets.to_string());
    let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!(
            "{cmake}\noption(ENABLE_API \"Enable public API\" OFF)\nif(ENABLE_API AND \"$ENV{{API_MODE}}\" STREQUAL \"enabled\")\ntarget_compile_definitions(example PUBLIC ENABLE_API=1)\nendif()\n"
        ),
    );
    write(
        temp.path(),
        "include/example.hpp",
        "#pragma once\n#ifndef ENABLE_API\n#error expected the preset enabled API\n#endif\ninline int answer(){return 42;}\n",
    );
    write(
        temp.path(),
        "src/main.cpp",
        "#include <example.hpp>\nint main(){return answer()==42?0:1;}\n",
    );
    assert_exit(
        &invoke(temp.path(), &["run", "--profile", "debug", "--suite", "headers"]),
        0,
    );
}
#[cfg(unix)]
#[test]
fn test_managed_root_and_build_ancestor_symlinks_are_rejected_before_writes() {
    for managed in ["build", "build/infra"] {
        let temp = fixture("");
        let path = temp.path().join(managed);
        fs::create_dir_all(path.parent().expect("parent")).expect("parents");
        std::os::unix::fs::symlink(temp.path().join("src"), &path).expect("managed symlink");
        let before: Vec<_> = walkdir::WalkDir::new(temp.path().join("src"))
            .into_iter()
            .map(|entry| entry.expect("entry").into_path())
            .collect();
        assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
        let after: Vec<_> = walkdir::WalkDir::new(temp.path().join("src"))
            .into_iter()
            .map(|entry| entry.expect("entry").into_path())
            .collect();
        assert_eq!(
            after, before,
            "reject {managed} symlink before creating any source artifact"
        );
    }
}
#[test]
fn test_preset_include_cannot_read_configuration_from_digest_exclusions() {
    for excluded in [
        "build",
        "target",
        ".git",
        ".infra/.tmp",
        ".infra/transactions",
        ".infra/.transactions",
    ] {
        let temp = fixture("");
        let presets = fs::read_to_string(temp.path().join("CMakePresets.json")).expect("presets");
        write(temp.path(), &format!("{excluded}/base.json"), &presets);
        write(
            temp.path(),
            "CMakePresets.json",
            &json!({"version":6,"include":[format!("{excluded}/base.json")]}).to_string(),
        );
        assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
    }
}
#[cfg(unix)]
#[test]
fn test_configuration_symlinks_cannot_import_untracked_build_inputs() {
    for name in [
        ".infra/project.toml",
        ".infra/verify/config.toml",
        "CMakePresets.json",
        "CMakeLists.txt",
    ] {
        let temp = fixture("");
        let original = temp.path().join(name);
        let content = fs::read(&original).expect("input");
        let target = temp.path().join("build/untracked-input");
        fs::create_dir_all(target.parent().expect("parent")).expect("build");
        fs::write(&target, content).expect("input target");
        fs::remove_file(&original).expect("replace owned fixture input");
        std::os::unix::fs::symlink(&target, &original).expect("input symlink");
        assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
    }
}
#[cfg(unix)]
#[test]
fn test_managed_ancestor_symlink_cannot_redirect_default_error_report_into_source() {
    let temp = fixture("");
    std::os::unix::fs::symlink(temp.path().join("src"), temp.path().join("build")).expect("build symlink");
    let before: Vec<_> = walkdir::WalkDir::new(temp.path().join("src"))
        .into_iter()
        .map(|entry| entry.expect("entry").into_path())
        .collect();
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"))
        .args([
            "--project",
            temp.path().to_str().expect("path"),
            "prepare",
            "--profile",
            "debug",
        ])
        .output()
        .expect("CLI");
    assert_exit(&output, 2);
    let after: Vec<_> = walkdir::WalkDir::new(temp.path().join("src"))
        .into_iter()
        .map(|entry| entry.expect("entry").into_path())
        .collect();
    assert_eq!(
        before, after,
        "default error report must not write into aliased source tree"
    );
}
#[test]
fn test_normal_nested_build_directory_is_a_tracked_configuration_input() {
    let temp = fixture("");
    let presets = fs::read_to_string(temp.path().join("CMakePresets.json")).expect("presets");
    write(temp.path(), "src/build/presets.json", &presets);
    write(
        temp.path(),
        "CMakePresets.json",
        &json!({"version":6,"include":["src/build/presets.json"]}).to_string(),
    );
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
}
#[cfg(unix)]
#[test]
fn test_prepare_context_output_rejects_a_source_symlink_and_foreign_artifact_role() {
    let temp = fixture("");
    let original = fs::read(temp.path().join("src/main.cpp")).expect("source");
    fs::create_dir_all(temp.path().join("build")).expect("build");
    std::os::unix::fs::symlink(temp.path().join("src/main.cpp"), temp.path().join("build/context.json"))
        .expect("output symlink");
    assert_exit(
        &invoke(
            temp.path(),
            &[
                "prepare",
                "--profile",
                "debug",
                "--output",
                temp.path().join("build/context.json").to_str().expect("path"),
            ],
        ),
        2,
    );
    assert_eq!(fs::read(temp.path().join("src/main.cpp")).expect("source"), original);
    write(
        temp.path(),
        "build/runtime-snapshot.json",
        "{\"schema\":1,\"bundle_id\":\"foreign-role\"}",
    );
    assert_exit(
        &invoke(
            temp.path(),
            &[
                "prepare",
                "--profile",
                "debug",
                "--output",
                temp.path().join("build/runtime-snapshot.json").to_str().expect("path"),
            ],
        ),
        2,
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("build/runtime-snapshot.json")).expect("snapshot"),
        "{\"schema\":1,\"bundle_id\":\"foreign-role\"}"
    );
}
#[test]
fn test_source_exclusion_raw_aliases_cannot_hide_new_business_json() {
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("json");
    write(temp.path(), "business.json", "{\"orders\":[42]}");
    for alias in [
        "business.json/",
        "business.json//",
        "./business.json",
        "business//../business.json",
        "../business.json",
        "business.json\\",
    ] {
        let context = json!({"schema":1,"run_id":report["run_id"],"task_id":"verify","bundle_id":"development","project":temp.path(),"profile":"debug","source_digest":report["source_digest"],"config_digest":report["config_digest"],"report_root":report["report_root"],"source_excluded_paths":[alias]});
        write(temp.path(), "build/raw-alias-context.json", &context.to_string());
        let output = invoke(
            temp.path(),
            &[
                "--run-context",
                temp.path().join("build/raw-alias-context.json").to_str().expect("path"),
                "prepare",
                "--profile",
                "debug",
            ],
        );
        assert_exit(&output, 2);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("normalized"),
            "validate raw text before role/exists: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        fs::read_to_string(temp.path().join("business.json")).expect("business"),
        "{\"orders\":[42]}"
    );
}
#[test]
fn test_prepare_output_cannot_bypass_existing_role_with_a_trailing_slash() {
    let temp = fixture("");
    write(temp.path(), "build/business.json", "{\"orders\":[42]}");
    let alias = format!("{}/", temp.path().join("build/business.json").display());
    assert_exit(
        &invoke(temp.path(), &["prepare", "--profile", "debug", "--output", &alias]),
        2,
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("build/business.json")).expect("business"),
        "{\"orders\":[42]}"
    );
}
#[test]
fn test_untracked_toolchain_and_project_hook_inputs_are_rejected() {
    for key in [
        "CMAKE_TOOLCHAIN_FILE",
        "CMAKE_PROJECT_TOP_LEVEL_INCLUDES",
        "CMAKE_PROJECT_INCLUDE_BEFORE",
        "CMAKE_PROJECT_Fixture_INCLUDE",
        "CMAKE_USER_MAKE_RULES_OVERRIDE_CXX",
    ] {
        let temp = fixture("");
        write(
            temp.path(),
            "build/policy.cmake",
            "set(PROJECT_POLICY_ENABLED ON CACHE BOOL \"\" FORCE)\n",
        );
        let mut presets: Value =
            serde_json::from_slice(&fs::read(temp.path().join("CMakePresets.json")).expect("presets")).expect("json");
        presets["configurePresets"][0]["cacheVariables"][key] = json!("${sourceDir}/build/policy.cmake");
        write(temp.path(), "CMakePresets.json", &presets.to_string());
        assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
    }
    let temp = fixture("");
    write(
        temp.path(),
        "target/policy.cmake",
        "set(PROJECT_POLICY_ENABLED ON CACHE BOOL \"\" FORCE)\n",
    );
    let mut presets: Value =
        serde_json::from_slice(&fs::read(temp.path().join("CMakePresets.json")).expect("presets")).expect("json");
    presets["configurePresets"][0]["toolchainFile"] = json!("${sourceDir}/target/policy.cmake");
    write(temp.path(), "CMakePresets.json", &presets.to_string());
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
}
#[test]
fn test_local_cmake_transitive_includes_and_subdirectories_cannot_read_excluded_policy() {
    for command in [
        "include(\"${CMAKE_SOURCE_DIR}/build/policy.cmake\")",
        "add_subdirectory(\"${CMAKE_SOURCE_DIR}/build/local-policy\")",
    ] {
        let temp = fixture("");
        write(
            temp.path(),
            "build/policy.cmake",
            "set(PROJECT_POLICY_ENABLED ON CACHE BOOL \"\" FORCE)\n",
        );
        write(
            temp.path(),
            "build/local-policy/CMakeLists.txt",
            "set(PROJECT_POLICY_ENABLED ON CACHE BOOL \"\" FORCE)\n",
        );
        write(temp.path(), ".infra/cmake/toolchain.cmake", &format!("{command}\n"));
        let mut presets: Value =
            serde_json::from_slice(&fs::read(temp.path().join("CMakePresets.json")).expect("presets")).expect("json");
        presets["configurePresets"][0]["cacheVariables"]["CMAKE_TOOLCHAIN_FILE"] =
            json!("${sourceDir}/.infra/cmake/toolchain.cmake");
        write(temp.path(), "CMakePresets.json", &presets.to_string());
        assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
    }
}
#[cfg(unix)]
#[test]
fn test_toolchain_and_project_includes_reject_symlinks_even_to_tracked_files() {
    for key in ["CMAKE_TOOLCHAIN_FILE", "CMAKE_PROJECT_INCLUDE"] {
        let temp = fixture("");
        write(
            temp.path(),
            ".infra/cmake/actual.cmake",
            "set(PROJECT_POLICY_ENABLED ON CACHE BOOL \"\" FORCE)\n",
        );
        std::os::unix::fs::symlink(
            temp.path().join(".infra/cmake/actual.cmake"),
            temp.path().join(".infra/cmake/alias.cmake"),
        )
        .expect("config alias");
        let mut presets: Value =
            serde_json::from_slice(&fs::read(temp.path().join("CMakePresets.json")).expect("presets")).expect("json");
        presets["configurePresets"][0]["cacheVariables"][key] = json!("${sourceDir}/.infra/cmake/alias.cmake");
        write(temp.path(), "CMakePresets.json", &presets.to_string());
        assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
    }
}
#[test]
fn test_source_exclusion_rejects_windows_drive_absolute_and_relative_paths_on_every_host() {
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("json");
    for path in ["C:/report.json", "c:report.json", "Z:/report.json", "z:report.json"] {
        let context = json!({"schema":1,"run_id":report["run_id"],"task_id":"verify","bundle_id":"development","project":temp.path(),"profile":"debug","source_digest":report["source_digest"],"config_digest":report["config_digest"],"report_root":report["report_root"],"source_excluded_paths":[path]});
        write(temp.path(), "build/drive-context.json", &context.to_string());
        let output = invoke(
            temp.path(),
            &[
                "--run-context",
                temp.path().join("build/drive-context.json").to_str().expect("path"),
                "plan",
                "--profile",
                "debug",
            ],
        );
        assert_exit(&output, 2);
    }
}
#[test]
fn test_init_only_template_is_manifested_and_runs_real_build_and_tests() {
    use sha2::Digest;
    use sha2::Sha256;
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("conf");
    let manifest: Value =
        serde_json::from_slice(&fs::read(directory.join("manifest.json")).expect("manifest")).expect("JSON");
    let entries = manifest["project_files"]
        .as_array()
        .expect("verify must deliver init-only project config");
    let entry = entries
        .iter()
        .find(|entry| {
            entry["target"]
                .as_str()
                .is_some_and(|target| target.ends_with("config.toml"))
        })
        .expect("initial configuration entry");
    let content = fs::read(directory.join(entry["source"].as_str().expect("source"))).expect("template");
    assert_eq!(entry["sha256"], hex::encode(Sha256::digest(&content)));
    assert!(
        !manifest["files"]
            .as_array()
            .expect("managed files")
            .iter()
            .any(|managed| managed["target"] == entry["target"])
    );
    let temp = fixture("add_test(NAME example COMMAND example)");
    write(
        temp.path(),
        ".infra/verify/config.toml",
        std::str::from_utf8(&content).expect("TOML"),
    );
    assert_exit(&invoke(temp.path(), &["run", "--profile", "debug"]), 0);
}
#[test]
fn test_real_local_toolchain_and_computed_project_include_are_tracked_and_reusable() {
    let temp = fixture("add_test(NAME example COMMAND example)");
    write(
        temp.path(),
        ".infra/cmake policy/toolchain.cmake",
        "include(\"${CMAKE_CURRENT_LIST_DIR}/feature.cmake\")\n",
    );
    write(
        temp.path(),
        ".infra/cmake policy/feature.cmake",
        "set(TOOLCHAIN_FEATURE ON CACHE BOOL \"\" FORCE)\n",
    );
    write(
        temp.path(),
        ".infra/cmake policy/project.cmake",
        "set(PROJECT_INCLUDE_FEATURE ON)\n",
    );
    let mut presets: Value =
        serde_json::from_slice(&fs::read(temp.path().join("CMakePresets.json")).expect("presets")).expect("json");
    presets["configurePresets"][0]["toolchainFile"] = json!("${sourceDir}/.infra/cmake policy/toolchain.cmake");
    write(temp.path(), "CMakePresets.json", &presets.to_string());
    let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!(
            "{cmake}\nstring(CONCAT project_include \"${{CMAKE_CURRENT_LIST_DIR}}\" \"/.infra/cmake policy/project.cmake\")\ninclude(\"${{project_include}}\")\nif(NOT TOOLCHAIN_FEATURE OR NOT PROJECT_INCLUDE_FEATURE)\nmessage(FATAL_ERROR \"features must remain enabled\")\nendif()\nif(FALSE)\ninclude(\"${{CMAKE_SOURCE_DIR}}/build/unread-policy.cmake\")\nendif()\n"
        ),
    );
    write(
        temp.path(),
        "build/unread-policy.cmake",
        "message(FATAL_ERROR \"inactive branch must not execute\")\n",
    );
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("JSON");
    let context_path = report["data"]["build_context"].as_str().expect("context");
    let build: Value = serde_json::from_slice(&fs::read(context_path).expect("context")).expect("JSON");
    let inputs = build["configuration_inputs"].as_array().expect("proof");
    for name in ["toolchain.cmake", "feature.cmake", "project.cmake"] {
        assert!(
            inputs
                .iter()
                .any(|input| input["path"].as_str().is_some_and(|path| path.ends_with(name))
                    && input["kind"] == "project"),
            "missing actual {name}"
        );
    }
    assert!(!inputs.iter().any(|input| {
        input["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("unread-policy.cmake"))
    }));
    let ctx = json!({"schema":1,"run_id":report["run_id"],"task_id":"verify","bundle_id":"development","project":temp.path(),"profile":"debug","source_digest":report["source_digest"],"config_digest":report["config_digest"],"report_root":report["report_root"],"build_context":context_path});
    write(temp.path(), "build/tracked-context.json", &ctx.to_string());
    let context_arg = temp.path().join("build/tracked-context.json");
    let args = [
        "--run-context",
        context_arg.to_str().expect("path"),
        "prepare",
        "--profile",
        "debug",
    ];
    assert_exit(&invoke(temp.path(), &args), 0);
    write(
        temp.path(),
        ".infra/cmake policy/feature.cmake",
        "set(TOOLCHAIN_FEATURE OFF CACHE BOOL \"\" FORCE)\n",
    );
    assert_exit(&invoke(temp.path(), &args), 2);
}
#[test]
fn test_file_api_rejects_computed_untracked_include_before_build() {
    let temp = fixture("");
    write(temp.path(), "build/policy.cmake", "set(POLICY_FEATURE ON)\n");
    let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!(
            "{cmake}\nstring(CONCAT policy_file \"${{CMAKE_CURRENT_LIST_DIR}}\" \"/build/policy.cmake\")\ninclude(\"${{policy_file}}\")\n"
        ),
    );
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("JSON");
    assert!(!report["commands"].as_array().expect("commands").iter().any(|command| {
        command["args"]
            .as_array()
            .is_some_and(|args| args.contains(&json!("--build")))
    }));
}
#[test]
fn test_actual_cmake_cache_mutation_invalidates_prepared_configuration_proof() {
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("JSON");
    let ctx = json!({"schema":1,"run_id":report["run_id"],"task_id":"verify","bundle_id":"development","project":temp.path(),"profile":"debug","source_digest":report["source_digest"],"config_digest":report["config_digest"],"report_root":report["report_root"],"build_context":report["data"]["build_context"]});
    write(temp.path(), "build/cache-run-context.json", &ctx.to_string());
    let path = temp.path().join("build/infra/debug/CMakeCache.txt");
    let cache = fs::read_to_string(&path).expect("cache");
    fs::write(path, format!("{cache}\nUNTRACKED_CACHE_CHANGE:BOOL=ON\n")).expect("mutate cache");
    assert_exit(
        &invoke(
            temp.path(),
            &[
                "--run-context",
                temp.path().join("build/cache-run-context.json").to_str().expect("path"),
                "prepare",
                "--profile",
                "debug",
            ],
        ),
        2,
    );
}
#[test]
fn test_handwritten_cmake_system_basename_cannot_claim_engine_generated_owner() {
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    write(
        temp.path(),
        "build/infra/debug/CMakeFiles/custom/CMakeSystem.cmake",
        "set(POLICY_ENABLED ON CACHE BOOL \"\" FORCE)\n",
    );
    let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!(
            "{cmake}\nstring(CONCAT custom_policy \"${{CMAKE_BINARY_DIR}}\" \"/CMakeFiles/custom/CMakeSystem.cmake\")\ninclude(\"${{custom_policy}}\")\n"
        ),
    );
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
}
#[test]
fn test_preexisting_engine_generated_policy_modification_is_rejected_before_configure() {
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("JSON");
    let context: Value =
        serde_json::from_slice(&fs::read(report["data"]["build_context"].as_str().expect("context")).expect("context"))
            .expect("JSON");
    let system = Path::new(
        context["configuration_inputs"]
            .as_array()
            .expect("inputs")
            .iter()
            .find(|input| {
                input["kind"] == "cmake-generated"
                    && input["path"]
                        .as_str()
                        .is_some_and(|path| path.ends_with("CMakeSystem.cmake"))
            })
            .expect("system")["path"]
            .as_str()
            .expect("path"),
    );
    let original = fs::read_to_string(system).expect("system");
    let changed = format!("{original}\nset(UNTRACKED_POLICY_ACCEPTED ON CACHE BOOL \"\" FORCE)\n");
    fs::write(system, &changed).expect("modify generated artifact");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 2);
    assert_eq!(fs::read_to_string(system).expect("preserve modified file"), changed);
    assert!(
        !fs::read_to_string(temp.path().join("build/infra/debug/CMakeCache.txt"))
            .expect("cache")
            .contains("UNTRACKED_POLICY_ACCEPTED:BOOL=ON")
    );
}
#[test]
fn test_explicit_system_cmake_module_is_accepted_as_tool_input() {
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("JSON");
    let context: Value =
        serde_json::from_slice(&fs::read(report["data"]["build_context"].as_str().expect("context")).expect("context"))
            .expect("JSON");
    let index: Value =
        serde_json::from_slice(&fs::read(context["file_api_index"].as_str().expect("index")).expect("index"))
            .expect("JSON");
    let module =
        Path::new(index["cmake"]["paths"]["root"].as_str().expect("CMake root")).join("Modules/GNUInstallDirs.cmake");
    let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!("{cmake}\ninclude(\"{}\")\n", module.display()),
    );
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
}
#[test]
fn test_unrelated_escaped_cmake_parentheses_do_not_change_input_discovery() {
    for statement in ["set(UNRELATED a\\(b)", "set(UNRELATED a\\)b)"] {
        let temp = fixture("");
        let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
        write(temp.path(), "CMakeLists.txt", &format!("{cmake}\n{statement}\n"));
        assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    }
}
#[test]
fn test_cache_change_during_suite_invalidates_current_success_report() {
    let temp =
        fixture("add_test(NAME change_cache COMMAND ${CMAKE_COMMAND} -P ${CMAKE_SOURCE_DIR}/tests/change-cache.cmake)");
    write(
        temp.path(),
        "tests/change-cache.cmake",
        &format!(
            "file(APPEND \"{}\" \"\\n# changed during test\\n\")\n",
            temp.path().join("build/infra/debug/CMakeCache.txt").display()
        ),
    );
    assert_exit(
        &invoke(temp.path(), &["run", "--profile", "debug", "--suite", "test"]),
        2,
    );
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("JSON");
    assert_eq!(report["status"], "error");
}
#[test]
fn test_failed_configure_can_retry_using_provenance_from_the_owned_attempt() {
    let temp = fixture("");
    let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!("{cmake}\nmessage(FATAL_ERROR \"intentional configuration failure\")\n"),
    );
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 1);
    let state_path = temp.path().join("build/infra/debug/.cpp-infra-build-state.json");
    let state: Value = serde_json::from_slice(&fs::read(&state_path).expect("state")).expect("JSON");
    assert!(state["fingerprint"].is_null());
    assert!(
        !state["cmake_engine_proofs"]
            .as_array()
            .expect("attempt proof")
            .is_empty()
    );
    write(temp.path(), "CMakeLists.txt", &cmake);
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
}
#[test]
fn test_engine_proof_migrates_verified_legacy_state_and_rejects_foreign_host() {
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let path = temp.path().join("build/infra/debug/.cpp-infra-build-state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).expect("state")).expect("JSON");
    state
        .as_object_mut()
        .expect("state object")
        .remove("cmake_engine_proofs");
    fs::write(&path, state.to_string()).expect("legacy state");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let original = fs::read_to_string(&path).expect("state");
    let mut state: Value = serde_json::from_str(&original).expect("JSON");
    state["cmake_engine_proofs"][0]["engine"]["host_os"] = json!("different-host");
    fs::write(&path, state.to_string()).expect("host metadata");
    let output = invoke(temp.path(), &["prepare", "--profile", "debug"]);
    assert_exit(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("host changed"));
    fs::write(&path, original).expect("restore verified proof");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
}
#[cfg(unix)]
#[test]
fn test_non_symlink_parent_relative_system_module_uses_actual_tool_owner() {
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("JSON");
    let context: Value =
        serde_json::from_slice(&fs::read(report["data"]["build_context"].as_str().expect("context")).expect("context"))
            .expect("JSON");
    let index: Value =
        serde_json::from_slice(&fs::read(context["file_api_index"].as_str().expect("index")).expect("index"))
            .expect("JSON");
    let module =
        Path::new(index["cmake"]["paths"]["root"].as_str().expect("root")).join("Modules/GNUInstallDirs.cmake");
    let parents = temp
        .path()
        .components()
        .filter(|part| matches!(part, std::path::Component::Normal(_)))
        .count();
    let relative = format!(
        "{}{}",
        "../".repeat(parents),
        module.strip_prefix("/").expect("absolute module").display()
    );
    let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!("{cmake}\ninclude(\"${{CMAKE_CURRENT_LIST_DIR}}/{relative}\")\n"),
    );
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
}

#[test]
fn test_malformed_presets_and_absolute_foreign_binary_dir_are_rejected() {
    let temp = fixture("");
    let original = fs::read_to_string(temp.path().join("CMakePresets.json")).expect("presets");
    write(temp.path(), "CMakePresets.json", "{invalid-json");
    let output = invoke(temp.path(), &["prepare", "--profile", "debug"]);
    assert_exit(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("presets:"));
    let outside = tempfile::tempdir().expect("outside project");
    let mut presets: Value = serde_json::from_str(&original).expect("presets");
    presets["configurePresets"][0]["binaryDir"] = json!(outside.path());
    write(temp.path(), "CMakePresets.json", &presets.to_string());
    let output = invoke(temp.path(), &["prepare", "--profile", "debug"]);
    assert_exit(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("binaryDir outside project"));
    assert_eq!(fs::read_dir(outside.path()).expect("outside unchanged").count(), 0);
}
#[test]
fn test_corrupt_stored_context_is_reported_as_a_protocol_error() {
    let temp = fixture("");
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let report: Value =
        serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("JSON");
    let context = json!({"schema":1,"run_id":report["run_id"],"task_id":"verify-test","bundle_id":"development","project":temp.path(),"profile":"debug","config_digest":report["config_digest"],"source_digest":report["source_digest"],"report_root":report["report_root"],"build_context":report["data"]["build_context"]});
    fs::write(
        report["data"]["build_context"].as_str().expect("context"),
        "{broken-json",
    )
    .expect("corrupt fixture artifact");
    write(temp.path(), "build/reuse.json", &context.to_string());
    let output = invoke(
        temp.path(),
        &[
            "--run-context",
            temp.path().join("build/reuse.json").to_str().expect("path"),
            "prepare",
            "--profile",
            "debug",
        ],
    );
    assert_exit(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("build context:"));
}
#[test]
fn test_headers_preserve_existing_top_level_include_and_bracket_arguments() {
    let temp = fixture("");
    write(
        temp.path(),
        "cmake/profile-hook.cmake",
        "set(PROFILE_HOOK_VALUE 42 CACHE STRING \"fixture\" FORCE)\n#[=[ a bracket comment containing unmatched ( ]=]\nset(IGNORED [==[literal ]=] parentheses ( )]==])\n",
    );
    let mut presets: Value =
        serde_json::from_slice(&fs::read(temp.path().join("CMakePresets.json")).expect("presets")).expect("JSON");
    presets["configurePresets"][0]["cacheVariables"]["CMAKE_PROJECT_TOP_LEVEL_INCLUDES"] =
        json!("${sourceDir}/cmake/profile-hook.cmake");
    write(temp.path(), "CMakePresets.json", &presets.to_string());
    let cmake = fs::read_to_string(temp.path().join("CMakeLists.txt")).expect("CMake");
    write(
        temp.path(),
        "CMakeLists.txt",
        &format!(
            "{cmake}\nif(NOT PROFILE_HOOK_VALUE EQUAL 42)\n message(FATAL_ERROR \"profile hook missing\")\nendif()\ntarget_compile_definitions(example PUBLIC PROFILE_HOOK_VALUE=${{PROFILE_HOOK_VALUE}})\n"
        ),
    );
    write(
        temp.path(),
        "include/example.hpp",
        "#pragma once\n#ifndef PROFILE_HOOK_VALUE\n#error missing original hook usage requirement\n#endif\nstatic_assert(PROFILE_HOOK_VALUE == 42);\n",
    );
    assert_exit(
        &invoke(temp.path(), &["run", "--profile", "debug", "--suite", "headers"]),
        0,
    );
}
#[test]
fn test_fuzz_reports_requested_target_missing_from_real_codemodel() {
    let temp = fixture("");
    clang_profile(&temp, "-fsanitize=fuzzer-no-link,address");
    write(temp.path(), "tests/corpus/seed", "seed");
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['fuzz']\n[fuzz]\nmode='build-only'\n[[fuzz.targets]]\ntarget='absent'\nseconds=1\nmax_len=16\ncorpus='tests/corpus'\ntimeout_seconds=3\n",
    );
    let output = invoke(temp.path(), &["run", "--profile", "debug"]);
    assert_exit(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("fuzz target missing: absent"));
}
#[cfg(unix)]
#[test]
fn test_malformed_ctest_protocol_outputs_do_not_pass_the_gate() {
    use std::os::unix::fs::PermissionsExt;
    for (selection, expected) in [
        ("invalid-json", "CTest selection:"),
        ("{\"tests\":[{}]}", "CTest JUnit:"),
    ] {
        let temp = fixture("add_test(NAME example COMMAND example)");
        let tools = tempfile::tempdir().expect("mock CTest protocol");
        write(
            tools.path(),
            "ctest",
            &format!(
                "#!/bin/sh\nfor arg in \"$@\"; do\n if [ \"$arg\" = '--show-only=json-v1' ]; then printf '%s' '{selection}'; exit 0; fi\ndone\nwhile [ \"$#\" -gt 0 ]; do\n if [ \"$1\" = '--output-junit' ]; then shift; printf '%s' '<broken' > \"$1\"; exit 0; fi\n shift\ndone\nexit 99\n"
            ),
        );
        fs::set_permissions(tools.path().join("ctest"), fs::Permissions::from_mode(0o755)).expect("executable mock");
        let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"))
            .arg("--project")
            .arg(temp.path())
            .arg("--report")
            .arg(temp.path().join("build/report.json"))
            .args(["run", "--profile", "debug", "--suite", "test"])
            .env(
                "PATH",
                format!("{}:{}", tools.path().display(), std::env::var("PATH").expect("PATH")),
            )
            .output()
            .expect("CLI");
        assert_exit(&output, 2);
        assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
    }
}

#[test]
fn test_suite_completion_revalidates_complete_build_state_and_owner() {
    for reuse in [false, true] {
        let temp =
            fixture("add_test(NAME mutate COMMAND ${CMAKE_COMMAND} -P ${CMAKE_SOURCE_DIR}/tests/mutate-state.cmake)");
        write(
            temp.path(),
            "tests/mutate-state.cmake",
            "set(path \"${CMAKE_CURRENT_LIST_DIR}/../build/infra/debug/$ENV{MUTATION_FILE}\")\nfile(READ \"${path}\" content)\nstring(JSON content SET \"${content}\" \"$ENV{MUTATION_KEY}\" \"$ENV{MUTATION_VALUE}\")\nfile(WRITE \"${path}\" \"${content}\")\n",
        );
        assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
        let state_path = temp.path().join("build/infra/debug/.cpp-infra-build-state.json");
        let marker_path = temp.path().join("build/infra/debug/.cpp-infra-project.json");
        let initial_state = fs::read(&state_path).expect("state");
        let initial_marker = fs::read(&marker_path).expect("owner");
        let report: Value =
            serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("JSON");
        let context = json!({"schema":1,"run_id":report["run_id"],"task_id":"state-barrier","bundle_id":"development","project":temp.path(),"profile":"debug","config_digest":report["config_digest"],"source_digest":report["source_digest"],"report_root":report["report_root"],"build_context":report["data"]["build_context"]});
        write(temp.path(), "build/state-run.json", &context.to_string());
        for (file, key, value) in [
            (
                ".cpp-infra-build-state.json",
                "fingerprint",
                json!("changed-during-test"),
            ),
            (".cpp-infra-build-state.json", "status", json!("dependency-updating")),
            (".cpp-infra-build-state.json", "schema", json!(2)),
            (".cpp-infra-build-state.json", "project", json!("/other-project")),
            (".cpp-infra-build-state.json", "profile", json!("other-profile")),
            (".cpp-infra-build-state.json", "source_digest", json!("changed-source")),
            (".cpp-infra-build-state.json", "config_digest", json!("changed-config")),
            (
                ".cpp-infra-build-state.json",
                "source_excluded_paths",
                json!(["other.json"]),
            ),
            (".cpp-infra-build-state.json", "configuration_inputs", json!([])),
            (
                ".cpp-infra-build-state.json",
                "cmake_cache_digest",
                json!("changed-cache-proof"),
            ),
            (".cpp-infra-build-state.json", "cmake_engine_proofs", json!([])),
            (".cpp-infra-project.json", "schema", json!(2)),
            (".cpp-infra-project.json", "project", json!("/other-project")),
        ] {
            fs::write(&state_path, &initial_state).expect("restore state");
            fs::write(&marker_path, &initial_marker).expect("restore owner");
            let mut command = Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"));
            command
                .arg("--project")
                .arg(temp.path())
                .arg("--report")
                .arg(temp.path().join("build/report.json"));
            if reuse {
                command
                    .arg("--run-context")
                    .arg(temp.path().join("build/state-run.json"));
            }
            let output = command
                .args(["run", "--profile", "debug", "--suite", "test,headers"])
                .env("MUTATION_FILE", file)
                .env("MUTATION_KEY", key)
                .env("MUTATION_VALUE", value.to_string())
                .output()
                .expect("CLI");
            assert_exit(&output, 2);
            let report: Value =
                serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report"))
                    .expect("JSON");
            assert_eq!(report["status"], "error", "{file}:{key}");
            let suites = report["data"]["suites"].as_array().expect("suites");
            assert_eq!(
                suites.iter().find(|suite| suite["suite"] == "test").expect("test")["status"],
                "failed",
                "reuse={reuse} {file}:{key}: {report}"
            );
            assert_eq!(
                suites
                    .iter()
                    .find(|suite| suite["suite"] == "headers")
                    .expect("headers")["status"],
                "blocked"
            );
            let actual: Value = serde_json::from_slice(
                &fs::read(temp.path().join("build/infra/debug").join(file)).expect("mutated state"),
            )
            .expect("JSON");
            assert_eq!(actual[key], value, "preserve mutation evidence");
        }
    }
}
#[cfg(unix)]
#[test]
fn test_prepare_completion_revalidates_state_after_tool_consumption() {
    use std::os::unix::fs::PermissionsExt;
    let temp = fixture("");
    let tools = tempfile::tempdir().expect("protocol shim");
    let actual_ninja = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|path| path.join("ninja"))
        .find(|path| path.is_file())
        .expect("real Ninja")
        .canonicalize()
        .expect("real path");
    write(
        tools.path(),
        "ninja",
        &format!(
            "#!/usr/bin/python3\nimport json,os,sys\np=os.environ['MUTATION_STATE']\nif os.path.isfile(p):\n with open(p) as f: state=json.load(f)\n if state.get('status')=='built':\n  state['status']='changed-after-build'\n  with open(p,'w') as f: json.dump(state,f)\nos.execv({},[{}]+sys.argv[1:])\n",
            json!(actual_ninja),
            json!(actual_ninja)
        ),
    );
    fs::set_permissions(tools.path().join("ninja"), fs::Permissions::from_mode(0o755)).expect("executable shim");
    for reuse in [false, true] {
        assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
        let report: Value =
            serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("JSON");
        let context = json!({"schema":1,"run_id":report["run_id"],"task_id":"prepare-state-barrier","bundle_id":"development","project":temp.path(),"profile":"debug","config_digest":report["config_digest"],"source_digest":report["source_digest"],"report_root":report["report_root"],"build_context":report["data"]["build_context"]});
        write(temp.path(), "build/prepare-state-run.json", &context.to_string());
        let mut command = Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"));
        command
            .arg("--project")
            .arg(temp.path())
            .arg("--report")
            .arg(temp.path().join("build/report.json"));
        if reuse {
            command
                .arg("--run-context")
                .arg(temp.path().join("build/prepare-state-run.json"));
        }
        let output = command
            .args(["prepare", "--profile", "debug"])
            .env(
                "PATH",
                format!("{}:{}", tools.path().display(), std::env::var("PATH").expect("PATH")),
            )
            .env(
                "MUTATION_STATE",
                temp.path().join("build/infra/debug/.cpp-infra-build-state.json"),
            )
            .output()
            .expect("CLI");
        assert_exit(&output, 2);
        let report: Value =
            serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report")).expect("JSON");
        assert_eq!(report["status"], "error");
    }
}

#[cfg(unix)]
#[test]
fn test_package_rejects_raw_duplicate_names_and_preserves_sfx_zip64_support() {
    use std::fs::File;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    use zip::write::SimpleFileOptions;
    fn zip64_directory(mut bytes: Vec<u8>) -> Vec<u8> {
        let end = bytes.len() - 22;
        let count = u64::from(u16::from_le_bytes(
            bytes[end + 10..end + 12].try_into().expect("entry count"),
        ));
        let size = u64::from(u32::from_le_bytes(
            bytes[end + 12..end + 16].try_into().expect("directory size"),
        ));
        let offset = u64::from(u32::from_le_bytes(
            bytes[end + 16..end + 20].try_into().expect("directory offset"),
        ));
        bytes.truncate(end);
        bytes.extend_from_slice(&0x06064b50u32.to_le_bytes());
        bytes.extend_from_slice(&44u64.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&[0; 8]);
        for value in [count, count, size, offset] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&0x07064b50u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&(end as u64).to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0x06054b50u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&[0xff; 12]);
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes
    }
    let temp = library_fixture(false);
    let tools = tempfile::tempdir().expect("isolated CPack protocol producer");
    let archive_path = tools.path().join("payload.zip");
    write(
        tools.path(),
        "cpack",
        &format!(
            "#!/usr/bin/python3\nimport pathlib,shutil,sys\nout=pathlib.Path(sys.argv[sys.argv.index('-B')+1]);out.mkdir(parents=True,exist_ok=True)\nshutil.copyfile({},out/'package.zip')\n",
            json!(archive_path)
        ),
    );
    fs::set_permissions(tools.path().join("cpack"), fs::Permissions::from_mode(0o755)).expect("executable CPack proxy");
    for (zip64, sfx) in [(false, false), (false, true), (true, false)] {
        for duplicate in [false, true] {
            let mut zip = zip::ZipWriter::new(File::create(&archive_path).expect("archive"));
            zip.start_file("include/example.hpp", SimpleFileOptions::default())
                .expect("first member");
            zip.write_all(b"#pragma once\n").expect("header");
            if duplicate {
                zip.start_file("include/another.hpp", SimpleFileOptions::default())
                    .expect("distinct raw name");
                zip.write_all(b"#pragma once\n").expect("header");
            }
            zip.finish().expect("finish");
            let mut bytes = fs::read(&archive_path).expect("archive");
            if duplicate {
                let original = b"include/another.hpp";
                let offsets: Vec<_> = bytes
                    .windows(original.len())
                    .enumerate()
                    .filter_map(|(offset, value)| (value == original).then_some(offset))
                    .collect();
                assert_eq!(offsets.len(), 2, "one local and one central raw filename");
                for offset in offsets {
                    bytes[offset..offset + original.len()].copy_from_slice(b"include/example.hpp");
                }
            }
            if zip64 {
                bytes = zip64_directory(bytes);
            }
            if sfx {
                bytes.splice(..0, b"fixture self-extractor prefix\n".iter().copied());
            }
            fs::write(&archive_path, bytes).expect("raw archive fixture");
            let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"))
                .arg("--project")
                .arg(temp.path())
                .arg("--report")
                .arg(temp.path().join("build/report.json"))
                .args(["run", "--profile", "debug", "--suite", "package"])
                .env(
                    "PATH",
                    format!("{}:{}", tools.path().display(), std::env::var("PATH").expect("PATH")),
                )
                .output()
                .expect("CLI");
            assert_exit(&output, i32::from(duplicate));
            let report: Value =
                serde_json::from_slice(&fs::read(temp.path().join("build/report.json")).expect("report"))
                    .expect("JSON");
            assert_eq!(
                report["status"],
                if duplicate { "failed" } else { "passed" },
                "zip64={zip64},sfx={sfx}"
            );
            if duplicate {
                assert!(String::from_utf8_lossy(&output.stderr).contains("duplicate archive entry"));
                let extracted = Path::new(report["data"]["build_context"].as_str().expect("context"))
                    .parent()
                    .expect("workspace")
                    .join("package/extracted-0/include/example.hpp");
                assert!(!extracted.exists(), "reject before writing folded member");
            }
        }
    }
}
