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
use sha2::Digest;
use sha2::Sha256;
use tempfile::TempDir;

fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directories");
    fs::write(path, text).expect("fixture file");
}

fn fixture() -> TempDir {
    let temp = tempfile::tempdir().expect("temporary project");
    write(
        temp.path(),
        ".infra/project.toml",
        "schema=1\nkind='application'\nsource_roots=['src','include']\ntest_roots=['tests']\n[profiles.debug]\nconfigure='debug'\nbuild='debug'\ntest='debug'\n",
    );
    write(
        temp.path(),
        ".infra/verify/config.toml",
        "schema=1\nrequired_suites=['build']\n",
    );
    write(temp.path(), "CMakePresets.json", &json!({"version":6,"configurePresets":[{"name":"debug","generator":"Ninja","binaryDir":"${sourceDir}/build/infra/debug","cacheVariables":{"CMAKE_BUILD_TYPE":"Debug","CMAKE_EXPORT_COMPILE_COMMANDS":"ON"}}],"buildPresets":[{"name":"debug","configurePreset":"debug"}],"testPresets":[{"name":"debug","configurePreset":"debug"}]}).to_string());
    write(
        temp.path(),
        "CMakeLists.txt",
        "cmake_minimum_required(VERSION 3.25)\nproject(ErrorPaths LANGUAGES CXX)\nadd_executable(example src/main.cpp)\nenable_testing()\nadd_test(NAME example COMMAND example)\n",
    );
    write(temp.path(), "src/main.cpp", "int main() { return 0; }\n");
    write(temp.path(), "include/api.hpp", "#pragma once\n");
    write(temp.path(), "tests/README", "test inputs\n");
    temp
}

fn invoke(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"))
        .arg("--project")
        .arg(root)
        .arg("--report")
        .arg(root.join("build/report.json"))
        .args(args)
        .output()
        .expect("public CLI")
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

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("JSON file")).expect("valid JSON")
}

fn assert_config_error(root: &Path, args: &[&str], message: &str) {
    assert_exit(&invoke(root, args), 2);
    let report = read_json(&root.join("build/report.json"));
    assert_eq!(report["status"], "error");
    assert_eq!(report["diagnostics"][0]["code"], "VERIFY_CONFIG");
    assert!(
        report["diagnostics"][0]["message"]
            .as_str()
            .expect("diagnostic")
            .contains(message),
        "expected {message:?}, report={report}"
    );
}

fn seed_context(root: &Path) -> Value {
    assert_exit(&invoke(root, &["plan", "--profile", "debug"]), 0);
    let report = read_json(&root.join("build/report.json"));
    json!({"schema":1,"run_id":"errors","task_id":"verify","bundle_id":"development","project":root,"profile":"debug","source_digest":report["source_digest"],"config_digest":report["config_digest"],"report_root":root.join("build/infra-runs/errors")})
}

#[test]
fn test_cli_usage_and_project_kind_errors() {
    let temp = fixture();
    let output = invoke(temp.path(), &[]);
    assert_exit(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("a command is required"));
    let output = invoke(temp.path(), &["--help"]);
    assert_exit(&output, 0);
    assert!(String::from_utf8_lossy(&output.stdout).contains("prepare"));
    let output = invoke(&temp.path().join("CMakeLists.txt"), &["plan", "--profile", "debug"]);
    assert_exit(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("--project must be a directory"));
}

#[test]
fn test_invalid_configuration_is_rejected_before_building() {
    for (configuration, message) in [
        (
            "schema=2\nrequired_suites=['build']\n",
            "unsupported configuration schema",
        ),
        (
            "schema=1\nrequired_suites=['build']\njobs=0\n",
            "jobs and timeouts must be positive",
        ),
        ("schema=1\nrequired_suites=[]\n", "required_suites must not be empty"),
        (
            "schema=1\nrequired_suites=['build','build']\n",
            "invalid or duplicate suite",
        ),
        (
            "schema=1\nrequired_suites=['imaginary']\n",
            "invalid or duplicate suite",
        ),
        (
            "schema=1\nrequired_suites=['build']\n[headers]\nroots=['tests']\n",
            "header roots must be within source_roots",
        ),
        (
            "schema=1\nrequired_suites=['build']\n[headers]\nroots=['include']\nexclude=['include/api.hpp']\n",
            "excluded header needs an exact exceptions host header",
        ),
        (
            "schema=1\nrequired_suites=['build']\n[install]\nexecutables=[]\n",
            "application install requires executables",
        ),
        (
            "schema=1\nrequired_suites=['build']\n[package]\npreset=''\nrequired_files=[]\n",
            "package requires preset and nonempty required_files",
        ),
        (
            "schema=1\nrequired_suites=['build']\n[sanitize]\nsanitizers=['thread','address']\n",
            "supported sanitizer sets",
        ),
        (
            "schema=1\nrequired_suites=['build']\n[fuzz]\nmode='unknown'\ntargets=[]\n",
            "invalid fuzz mode or empty targets",
        ),
        (
            "schema=1\nrequired_suites=['build']\n[fuzz]\nmode='smoke'\n[[fuzz.targets]]\ntarget='example'\nseconds=0\nmax_len=10\ncorpus='tests'\ntimeout_seconds=1\n",
            "invalid fuzz budget or duplicate target",
        ),
    ] {
        let temp = fixture();
        write(temp.path(), ".infra/verify/config.toml", configuration);
        assert_config_error(temp.path(), &["prepare", "--profile", "debug"], message);
        assert!(!temp.path().join("build/infra/debug").exists());
    }
}

#[test]
fn test_project_roots_require_disjoint_existing_safe_paths() {
    for (roots, message) in [
        ("['src','src/nested']", "overlapping project root"),
        ("['missing']", "configured path does not exist"),
        ("['../outside']", "unsafe project path"),
    ] {
        let temp = fixture();
        fs::create_dir_all(temp.path().join("src/nested")).expect("nested source");
        let project = fs::read_to_string(temp.path().join(".infra/project.toml"))
            .expect("project")
            .replace("['src','include']", roots);
        write(temp.path(), ".infra/project.toml", &project);
        assert_config_error(temp.path(), &["plan", "--profile", "debug"], message);
    }
}

#[test]
fn test_explicit_suite_selection_rejects_duplicate_unknown_and_unconfigured_suites() {
    let temp = fixture();
    for (suite, message) in [
        ("build,build", "duplicate requested suite"),
        ("imaginary", "unsupported or unconfigured suite"),
        ("install", "unsupported or unconfigured suite"),
    ] {
        assert_config_error(temp.path(), &["run", "--profile", "debug", "--suite", suite], message);
    }
    assert_config_error(
        temp.path(),
        &["plan", "--profile", "missing"],
        "unknown project profile",
    );
    assert!(!temp.path().join("build/infra/debug").exists());
}

#[test]
fn test_preset_protocol_and_inheritance_errors_prevent_commands() {
    for (mutation, message) in [
        (json!({"version":5}), "only CMake Presets schema 6"),
        (json!({"include":[12]}), "preset include must be a string"),
        (json!({"include":["CMakePresets.json"]}), "has a cycle"),
        (
            json!({"configurePresets":[{"name":"debug","inherits":"debug"}]}),
            "preset inheritance cycle",
        ),
        (
            json!({"configurePresets":[{"name":"debug","inherits":[12]}]}),
            "bad inherits",
        ),
        (
            json!({"configurePresets":[{"name":"debug","inherits":"missing"}]}),
            "missing configurePresets preset",
        ),
        (
            json!({"configurePresets":[{"name":"debug"},{"name":"debug"}]}),
            "duplicate preset",
        ),
        (
            json!({"configurePresets":[{"generator":"Ninja"}]}),
            "preset missing name",
        ),
        (
            json!({"buildPresets":[{"name":"debug","configurePreset":"other"}]}),
            "does not refer to its configure preset",
        ),
    ] {
        let temp = fixture();
        let mut presets = read_json(&temp.path().join("CMakePresets.json"));
        for (key, value) in mutation.as_object().expect("mutation") {
            presets[key] = value.clone();
        }
        write(temp.path(), "CMakePresets.json", &presets.to_string());
        assert_config_error(temp.path(), &["plan", "--profile", "debug"], message);
        assert_eq!(read_json(&temp.path().join("build/report.json"))["commands"], json!([]));
    }
}

#[test]
fn test_git_provenance_accepts_loose_packed_detached_and_gitdir_metadata() {
    let revision = "0123456789abcdef0123456789abcdef01234567";
    for mode in ["loose", "packed", "detached", "gitdir", "unsafe", "malformed"] {
        let temp = fixture();
        if mode == "gitdir" {
            write(temp.path(), ".git", "gitdir: build/git-metadata\n");
            write(temp.path(), "build/git-metadata/HEAD", &format!("{revision}\n"));
        } else {
            let head = match mode {
                "detached" => format!("{revision}\n"),
                "unsafe" => "ref: ../outside\n".into(),
                "malformed" => "not-a-revision\n".into(),
                _ => "ref: refs/heads/main\n".into(),
            };
            write(temp.path(), ".git/HEAD", &head);
            if mode == "loose" {
                write(temp.path(), ".git/refs/heads/main", &format!("{revision}\n"));
            }
            if mode == "packed" {
                write(
                    temp.path(),
                    ".git/packed-refs",
                    &format!("# pack-refs with: peeled fully-peeled sorted\n{revision} refs/heads/main\n"),
                );
            }
        }
        assert_exit(&invoke(temp.path(), &["plan", "--profile", "debug"]), 0);
        let report = read_json(&temp.path().join("build/report.json"));
        assert_eq!(
            report["source_revision"],
            if ["unsafe", "malformed"].contains(&mode) {
                Value::Null
            } else {
                json!(revision)
            },
            "metadata mode {mode}"
        );
    }
}

#[test]
fn test_run_context_rejects_missing_identity_and_unsafe_components() {
    for (key, value, message) in [
        ("run_id", Value::Null, "run context missing run_id"),
        ("task_id", json!("../escape"), "unsafe run context task_id"),
        ("bundle_id", json!("/absolute"), "unsafe run context bundle_id"),
        ("report_root", Value::Null, "run context report_root missing"),
        (
            "report_root",
            json!("/tmp/outside"),
            "run context report_root must be in project",
        ),
        (
            "build_context",
            json!(12),
            "run-context build_context must be an absolute path",
        ),
    ] {
        let temp = fixture();
        let mut context = seed_context(temp.path());
        context[key] = value;
        write(temp.path(), "build/run-context.json", &context.to_string());
        let path = temp.path().join("build/run-context.json");
        assert_config_error(
            temp.path(),
            &[
                "--run-context",
                path.to_str().expect("context path"),
                "plan",
                "--profile",
                "debug",
            ],
            message,
        );
    }
}

#[test]
fn test_malformed_and_missing_context_files_have_distinct_protocol_and_io_errors() {
    let temp = fixture();
    write(temp.path(), "build/run-context.json", "{broken");
    let path = temp.path().join("build/run-context.json");
    assert_config_error(
        temp.path(),
        &[
            "--run-context",
            path.to_str().expect("context path"),
            "plan",
            "--profile",
            "debug",
        ],
        "run-context.json",
    );
    let missing = temp.path().join("build/missing.json");
    assert_exit(
        &invoke(
            temp.path(),
            &[
                "--run-context",
                missing.to_str().expect("missing context"),
                "plan",
                "--profile",
                "debug",
            ],
        ),
        3,
    );
    let report = read_json(&temp.path().join("build/report.json"));
    assert_eq!(report["diagnostics"][0]["code"], "VERIFY_IO");
}

#[test]
fn test_runtime_snapshot_requires_bundle_and_unique_matching_tool_identity() {
    for (mutation, message) in [
        (json!({"schema":2}), "schema/bundle mismatch"),
        (json!({"bundle_id":"another"}), "schema/bundle mismatch"),
        (json!({"tools":null}), "tools missing"),
        (json!({"tools":[]}), "exactly one cpp-infra-verify"),
        (
            json!({"tools":[{"name":"cpp-infra-verify"},{"name":"cpp-infra-verify"}]}),
            "exactly one cpp-infra-verify",
        ),
        (
            json!({"tools":[{"name":"cpp-infra-verify","protocol":2}]}),
            "version/revision/protocol mismatch",
        ),
        (
            json!({"tools":[{"name":"cpp-infra-verify","protocol":1,"version":env!("CARGO_PKG_VERSION"),"revision":null}]}),
            "executable path missing",
        ),
    ] {
        let temp = fixture();
        let mut context = seed_context(temp.path());
        let mut snapshot = json!({"schema":1,"bundle_id":"development","tools":[]});
        for (key, value) in mutation.as_object().expect("snapshot mutation") {
            snapshot[key] = value.clone();
        }
        let text = snapshot.to_string();
        write(temp.path(), "build/infra-runs/errors/snapshot.json", &text);
        context["runtime_snapshot"] = json!(temp.path().join("build/infra-runs/errors/snapshot.json"));
        context["runtime_snapshot_digest"] = json!(hex::encode(Sha256::digest(text.as_bytes())));
        write(temp.path(), "build/run-context.json", &context.to_string());
        let path = temp.path().join("build/run-context.json");
        assert_config_error(
            temp.path(),
            &[
                "--run-context",
                path.to_str().expect("context path"),
                "plan",
                "--profile",
                "debug",
            ],
            message,
        );
    }
}

#[test]
fn test_foreign_and_unmanaged_build_directories_are_not_taken_over() {
    for (name, text, message) in [
        ("unexpected.txt", "user data", "unmarked nonempty build directory"),
        (
            ".cpp-infra-project.json",
            "{\"schema\":1,\"project\":\"/another-project\"}",
            "management marker mismatch",
        ),
    ] {
        let temp = fixture();
        write(temp.path(), &format!("build/infra/debug/{name}"), text);
        assert_config_error(temp.path(), &["prepare", "--profile", "debug"], message);
        assert_eq!(
            fs::read_to_string(temp.path().join(format!("build/infra/debug/{name}"))).expect("original data"),
            text
        );
        assert_eq!(read_json(&temp.path().join("build/report.json"))["commands"], json!([]));
    }
}

#[test]
fn test_missing_dependency_bundle_member_is_a_configuration_error() {
    let temp = fixture();
    let project = fs::read_to_string(temp.path().join(".infra/project.toml")).expect("project");
    write(
        temp.path(),
        ".infra/project.toml",
        &format!("{project}conan_context='debug'\n"),
    );
    let bundle = tempfile::tempdir().expect("empty tool bundle");
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"))
        .arg("--project")
        .arg(temp.path())
        .arg("--report")
        .arg(temp.path().join("build/report.json"))
        .args(["prepare", "--profile", "debug"])
        .env("CPP_INFRA_BIN_DIR", bundle.path())
        .output()
        .expect("CLI");
    assert_exit(&output, 2);
    let report = read_json(&temp.path().join("build/report.json"));
    assert!(
        report["diagnostics"][0]["message"]
            .as_str()
            .expect("diagnostic")
            .contains("bundle missing required tool")
    );
    assert_eq!(report["commands"], json!([]));
}

#[test]
fn test_prepared_file_api_replies_reject_corruption_before_reuse() {
    let temp = fixture();
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let report = read_json(&temp.path().join("build/report.json"));
    let stored_path = Path::new(report["data"]["build_context"].as_str().expect("prepared context"));
    let stored = read_json(stored_path);
    let context = json!({"schema":1,"run_id":"api-errors","task_id":"verify","bundle_id":"development","project":temp.path(),"profile":"debug","source_digest":report["source_digest"],"config_digest":report["config_digest"],"report_root":report["report_root"],"build_context":stored_path});
    write(temp.path(), "build/error-run.json", &context.to_string());
    let context_path = temp.path().join("build/error-run.json");
    let args = [
        "--run-context",
        context_path.to_str().expect("context path"),
        "prepare",
        "--profile",
        "debug",
    ];
    let index_path = Path::new(stored["file_api_index"].as_str().expect("index"));
    let index = read_json(index_path);
    let reply_dir = index_path.parent().expect("reply directory");
    let reply_path = |kind: &str| {
        let object = index["objects"]
            .as_array()
            .expect("reply objects")
            .iter()
            .find(|object| object["kind"] == kind)
            .expect("reply kind");
        reply_dir.join(object["jsonFile"].as_str().expect("reply name"))
    };
    let model_path = reply_path("codemodel");
    let tools_path = reply_path("toolchains");
    let target_path = reply_dir.join(
        read_json(&model_path)["configurations"][0]["targets"][0]["jsonFile"]
            .as_str()
            .expect("target reply"),
    );
    let cases = [
        (
            index_path.to_owned(),
            "/objects",
            json!([]),
            "File API has no codemodel reply",
        ),
        (
            index_path.to_owned(),
            "/objects/0/jsonFile",
            json!("../escaped.json"),
            "unsafe File API reference",
        ),
        (
            index_path.to_owned(),
            "/cmake/generator/name",
            json!("Another Generator"),
            "actual generator differs",
        ),
        (
            model_path.clone(),
            "/paths/source",
            Value::Null,
            "File API missing source",
        ),
        (
            model_path.clone(),
            "/paths/source",
            json!(temp.path().join("src")),
            "source root mismatch",
        ),
        (
            model_path.clone(),
            "/paths/build",
            Value::Null,
            "File API missing build",
        ),
        (
            model_path.clone(),
            "/paths/build",
            json!(temp.path()),
            "binary directory mismatch",
        ),
        (
            model_path.clone(),
            "/configurations",
            Value::Null,
            "File API missing configurations",
        ),
        (
            model_path.clone(),
            "/configurations",
            json!([]),
            "ambiguous File API configuration",
        ),
        (
            model_path.clone(),
            "/configurations/0/targets",
            Value::Null,
            "File API missing targets",
        ),
        (
            model_path.clone(),
            "/configurations/0/targets/0/jsonFile",
            Value::Null,
            "target missing jsonFile",
        ),
        (
            model_path.clone(),
            "/configurations/0/targets/0/jsonFile",
            json!("../target.json"),
            "unsafe target reply path",
        ),
        (tools_path.clone(), "/toolchains", json!([]), "CXX toolchain required"),
        (
            tools_path.clone(),
            "/toolchains/0/compiler/id",
            Value::Null,
            "CXX compiler missing id",
        ),
        (
            tools_path.clone(),
            "/toolchains/0/compiler/path",
            Value::Null,
            "CXX compiler missing path",
        ),
        (
            tools_path,
            "/toolchains/0/compiler/version",
            Value::Null,
            "CXX compiler missing version",
        ),
        (
            target_path,
            "/compileGroups",
            json!([]),
            "project has no buildable targets",
        ),
    ];
    for (path, pointer, value, message) in cases {
        let original = fs::read(&path).expect("original File API reply");
        let mut reply: Value = serde_json::from_slice(&original).expect("reply JSON");
        *reply.pointer_mut(pointer).expect("real CMake reply field") = value;
        fs::write(&path, reply.to_string()).expect("simulate damaged build artifact");
        assert_config_error(temp.path(), &args, message);
        assert_eq!(
            read_json(&temp.path().join("build/report.json"))["commands"],
            json!([]),
            "must reject reply before consuming prepared build"
        );
        fs::write(path, original).expect("restore build artifact");
    }
    assert_exit(&invoke(temp.path(), &args), 0);
    assert_eq!(
        read_json(&temp.path().join("build/report.json"))["data"]["reused_build"],
        true
    );
}

#[test]
fn test_prepared_context_and_build_state_must_keep_matching_identity() {
    let temp = fixture();
    assert_exit(&invoke(temp.path(), &["prepare", "--profile", "debug"]), 0);
    let report = read_json(&temp.path().join("build/report.json"));
    let stored_path = Path::new(report["data"]["build_context"].as_str().expect("prepared context"));
    let original = fs::read(stored_path).expect("prepared context bytes");
    let stored: Value = serde_json::from_slice(&original).expect("context JSON");
    let context = json!({"schema":1,"run_id":"state-errors","task_id":"verify","bundle_id":"development","project":temp.path(),"profile":"debug","source_digest":report["source_digest"],"config_digest":report["config_digest"],"report_root":report["report_root"],"build_context":stored_path});
    write(temp.path(), "build/error-run.json", &context.to_string());
    let context_path = temp.path().join("build/error-run.json");
    let args = [
        "--run-context",
        context_path.to_str().expect("context path"),
        "prepare",
        "--profile",
        "debug",
    ];
    let mut changed = stored.clone();
    changed["profile"] = json!("other");
    fs::write(stored_path, changed.to_string()).expect("changed context");
    assert_config_error(temp.path(), &args, "prepared build context does not match");
    fs::write(stored_path, &original).expect("restore context");
    let state_path = temp.path().join("build/infra/debug/.cpp-infra-build-state.json");
    let original_state = fs::read(&state_path).expect("build state");
    let mut state: Value = serde_json::from_slice(&original_state).expect("state JSON");
    state["fingerprint"] = json!("changed");
    fs::write(&state_path, state.to_string()).expect("changed build state");
    assert_config_error(temp.path(), &args, "prepared build state fingerprint changed");
    fs::write(&state_path, original_state).expect("restore state");
    let executable = Path::new(stored["targets"][0]["artifacts"][0].as_str().expect("artifact"));
    let bytes = fs::read(executable).expect("executable bytes");
    fs::remove_file(executable).expect("simulate missing disposable build artifact");
    assert_config_error(temp.path(), &args, "prepared build artifact missing");
    fs::write(executable, bytes).expect("restore build artifact");
}

#[test]
fn test_missing_cmake_records_spawn_failure_and_preserves_command_log() {
    let temp = fixture();
    let empty_path = tempfile::tempdir().expect("empty executable search path");
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"))
        .arg("--project")
        .arg(temp.path())
        .arg("--report")
        .arg(temp.path().join("build/report.json"))
        .args(["prepare", "--profile", "debug"])
        .env("PATH", empty_path.path())
        .output()
        .expect("CLI");
    assert_exit(&output, 3);
    let report = read_json(&temp.path().join("build/report.json"));
    assert_eq!(report["status"], "error");
    assert_eq!(report["diagnostics"][0]["code"], "VERIFY_IO");
    assert_eq!(report["commands"].as_array().expect("commands").len(), 1);
    let command = &report["commands"][0];
    assert_eq!(command["program"], "cmake");
    assert_eq!(command["args"], json!(["--version"]));
    assert!(command["exit_code"].is_null());
    let log = Path::new(report["report_root"].as_str().expect("report root"))
        .join(command["log"].as_str().expect("command log"));
    assert!(log.is_file(), "spawn failure keeps its diagnostic log artifact");
    let state = read_json(&temp.path().join("build/infra/debug/.cpp-infra-build-state.json"));
    assert_eq!(state["status"], "building");
    assert!(
        state["fingerprint"].is_null(),
        "spawn failure cannot leave a reusable success marker"
    );
}

#[cfg(unix)]
fn mock_dependency_bundle() -> TempDir {
    use std::os::unix::fs::PermissionsExt;

    let bundle = tempfile::tempdir().expect("mock sibling bundle");
    let producer = r#"#!/usr/bin/python3
# Protocol fixture only: this sibling does not run or represent real Conan.
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

args = sys.argv[1:]
def option(name):
    return args[args.index(name) + 1]

context = json.loads(Path(option('--run-context')).read_text())
root = Path(option('--project')).resolve()
binary = root / 'build/infra/debug'
generators = binary / 'generators'
generators.mkdir(parents=True, exist_ok=True)
toolchain = generators / 'conan_toolchain.cmake'
toolchain.write_text('set(CMAKE_CXX_STANDARD 20 CACHE STRING "fixture standard" FORCE)\n')
settings = {'compiler': 'gcc', 'compiler.version': subprocess.check_output(['/usr/bin/g++', '-dumpversion'], text=True).strip(), 'compiler.cppstd': 'gnu20', 'build_type': 'Debug'}
value = {'schema': 1, 'project': str(root), 'context': option('--context'), 'profile': context['profile'], 'source_digest': context['source_digest'], 'source_excluded_paths': context.get('source_excluded_paths', []), 'config_digest': context['config_digest'], 'baseline_id': 'mock-protocol-fixture', 'lock_digest': '1' * 64, 'graph_digest': '2' * 64, 'generators_dir': str(generators), 'generated_files': [{'path': str(toolchain), 'sha256': hashlib.sha256(toolchain.read_bytes()).hexdigest()}], 'fingerprint': '3' * 64, 'settings': settings}
report = dict(context, tool={'name':'cpp-infra-dependency','version':'0.1.0','revision':None}, status='passed', source_revision=None, started_at='2026-10-27T00:00:00Z', duration_ms=0, external_versions={}, commands=[], diagnostics=[], artifacts=[])
case = os.environ.get('MOCK_DEPENDENCY_CASE', 'valid')
if case == 'report-failed':
    report['status'] = 'failed'
elif case in ('schema', 'project', 'context', 'source_digest', 'config_digest', 'source_excluded_paths'):
    value[case] = 2 if case == 'schema' else ['unexpected.json'] if case == 'source_excluded_paths' else 'incorrect'
elif case == 'missing-generators':
    del value['generators_dir']
elif case == 'outside-generators':
    value['generators_dir'] = str(root / 'tests')
elif case == 'foreign-profile-generators':
    other = root / 'build/infra/foreign/generators'
    other.mkdir(parents=True)
    value['generators_dir'] = str(other)
    value['generated_files'] = []
elif case == 'missing-files':
    del value['generated_files']
elif case == 'missing-file-path':
    del value['generated_files'][0]['path']
elif case == 'missing-file':
    value['generated_files'][0]['path'] = str(generators / 'missing.cmake')
elif case == 'file-owner':
    value['generated_files'][0]['path'] = str(root / 'CMakeLists.txt')
elif case == 'file-hash':
    value['generated_files'][0]['sha256'] = '0' * 64
elif case == 'bad-fingerprint':
    value['fingerprint'] = 'invalid'
elif case == 'missing-settings':
    del value['settings']
elif case == 'compiler':
    settings['compiler'] = 'clang'
elif case == 'compiler.version':
    settings['compiler.version'] = '999'
elif case == 'compiler.cppstd':
    settings['compiler.cppstd'] = '17'
elif case == 'build_type':
    settings['build_type'] = 'Release'
elif case == 'compiler.runtime':
    settings['compiler.runtime'] = 'dynamic'
elif case == 'producer-fails':
    sys.exit(7)
output = Path(option('--output'))
output.parent.mkdir(parents=True, exist_ok=True)
if case != 'no-output':
    output.write_text('{broken' if case == 'invalid-json' else json.dumps(value))
Path(option('--report')).write_text(json.dumps(report))
"#;
    write(bundle.path(), "cpp-infra-dependency", producer);
    fs::set_permissions(
        bundle.path().join("cpp-infra-dependency"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("executable mock sibling producer");
    bundle
}

#[cfg(unix)]
fn dependency_fixture() -> TempDir {
    let temp = fixture();
    let project = fs::read_to_string(temp.path().join(".infra/project.toml")).expect("project");
    write(
        temp.path(),
        ".infra/project.toml",
        &format!("{project}conan_context='debug'\n"),
    );
    let mut presets = read_json(&temp.path().join("CMakePresets.json"));
    presets["configurePresets"][0]["toolchainFile"] =
        json!("${sourceDir}/build/infra/debug/generators/conan_toolchain.cmake");
    presets["configurePresets"][0]["cacheVariables"]["CMAKE_CXX_COMPILER"] = json!("/usr/bin/g++");
    presets["configurePresets"][0]["cacheVariables"]["CMAKE_CXX_STANDARD"] = json!("20");
    write(temp.path(), "CMakePresets.json", &presets.to_string());
    temp
}

#[cfg(unix)]
fn invoke_mock_dependency(root: &Path, bundle: &Path, case: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cpp-infra-verify"))
        .arg("--project")
        .arg(root)
        .arg("--report")
        .arg(root.join("build/report.json"))
        .args(args)
        .env("CPP_INFRA_BIN_DIR", bundle)
        .env("MOCK_DEPENDENCY_CASE", case)
        .output()
        .expect("CLI with explicit mock sibling producer")
}

#[cfg(unix)]
#[test]
fn test_mock_dependency_protocol_supports_real_cmake_prepare_and_verified_reuse() {
    let bundle = mock_dependency_bundle();
    let temp = dependency_fixture();
    assert_exit(
        &invoke_mock_dependency(
            temp.path(),
            bundle.path(),
            "valid",
            &["--offline", "prepare", "--profile", "debug"],
        ),
        0,
    );
    let report = read_json(&temp.path().join("build/report.json"));
    let stored_path = Path::new(report["data"]["build_context"].as_str().expect("prepared context"));
    let stored = read_json(stored_path);
    assert!(
        stored["configuration_inputs"]
            .as_array()
            .expect("configuration proof")
            .iter()
            .any(|input| input["kind"] == "dependency-generated"
                && input["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("conan_toolchain.cmake")))
    );
    assert_eq!(stored["compiler"]["cppstd"], "20");
    assert!(Path::new(stored["targets"][0]["artifacts"][0].as_str().expect("real executable")).is_file());
    let dependency_command = &report["commands"][0];
    assert!(
        dependency_command["args"]
            .as_array()
            .expect("forwarded arguments")
            .contains(&json!("--offline"))
    );
    let context = json!({"schema":1,"run_id":"dependency-reuse","task_id":"verify","bundle_id":"development","project":temp.path(),"profile":"debug","source_digest":report["source_digest"],"config_digest":report["config_digest"],"report_root":report["report_root"],"build_context":stored_path});
    write(temp.path(), "build/dependency-reuse.json", &context.to_string());
    let context_path = temp.path().join("build/dependency-reuse.json");
    let args = [
        "--run-context",
        context_path.to_str().expect("context path"),
        "prepare",
        "--profile",
        "debug",
    ];
    assert_exit(
        &invoke_mock_dependency(temp.path(), bundle.path(), "producer-fails", &args),
        0,
    );
    let reused = read_json(&temp.path().join("build/report.json"));
    assert_eq!(reused["data"]["reused_build"], true);
    assert!(
        reused["commands"]
            .as_array()
            .expect("reuse commands")
            .iter()
            .all(|command| command["program"] == "ninja")
    );
    let toolchain = temp.path().join("build/infra/debug/generators/conan_toolchain.cmake");
    fs::write(toolchain, "set(CMAKE_CXX_STANDARD 17)\n").expect("mutated generated toolchain");
    assert_exit(&invoke_mock_dependency(temp.path(), bundle.path(), "valid", &args), 2);
    let failed = read_json(&temp.path().join("build/report.json"));
    assert!(
        failed["diagnostics"][0]["message"]
            .as_str()
            .expect("diagnostic")
            .contains("dependency generated file fingerprint mismatch")
    );
}

#[cfg(unix)]
#[test]
fn test_mock_dependency_protocol_rejects_invalid_responses_and_toolchain_settings() {
    let bundle = mock_dependency_bundle();
    for (case, code, message) in [
        ("report-failed", 2, "dependency context/report fingerprint mismatch"),
        ("schema", 2, "dependency context/report fingerprint mismatch"),
        ("project", 2, "dependency context/report fingerprint mismatch"),
        ("context", 2, "dependency context/report fingerprint mismatch"),
        ("source_digest", 2, "dependency context/report fingerprint mismatch"),
        ("config_digest", 2, "dependency context/report fingerprint mismatch"),
        (
            "source_excluded_paths",
            2,
            "dependency context/report fingerprint mismatch",
        ),
        ("missing-generators", 2, "dependency generators_dir missing"),
        ("outside-generators", 2, "dependency generators outside build/infra"),
        (
            "foreign-profile-generators",
            2,
            "dependency generated configuration must belong to this binary directory",
        ),
        ("missing-files", 2, "dependency generated_files missing"),
        ("missing-file-path", 2, "dependency generated file missing path"),
        ("file-owner", 2, "dependency generated file fingerprint mismatch"),
        ("file-hash", 2, "dependency generated file fingerprint mismatch"),
        (
            "bad-fingerprint",
            2,
            "dependency configuration proof fingerprint missing or invalid",
        ),
        ("missing-settings", 2, "dependency context settings missing"),
        ("compiler", 2, "configured CMake compiler does not match Conan compiler"),
        (
            "compiler.version",
            2,
            "configured CMake compiler version does not match Conan compiler.version",
        ),
        (
            "compiler.cppstd",
            2,
            "configured CMake C++ standard does not match Conan compiler.cppstd",
        ),
        (
            "build_type",
            2,
            "configured CMake build_type does not match Conan build_type",
        ),
        ("compiler.runtime", 2, "cannot verify CMake MSVC runtime against Conan"),
        ("invalid-json", 2, "dependency-context.json"),
        ("missing-file", 3, ""),
        ("no-output", 3, ""),
        ("producer-fails", 1, "exited Some(7)"),
    ] {
        let temp = dependency_fixture();
        assert_exit(
            &invoke_mock_dependency(temp.path(), bundle.path(), case, &["prepare", "--profile", "debug"]),
            code,
        );
        let report = read_json(&temp.path().join("build/report.json"));
        assert_eq!(
            report["status"],
            if code == 1 { "failed" } else { "error" },
            "mock case {case}"
        );
        assert_eq!(
            report["diagnostics"][0]["code"],
            if code == 3 {
                "VERIFY_IO"
            } else if code == 1 {
                "VERIFY_GATE"
            } else {
                "VERIFY_CONFIG"
            },
            "mock case {case}"
        );
        assert!(
            report["diagnostics"][0]["message"]
                .as_str()
                .expect("diagnostic")
                .contains(message),
            "mock case {case}: {report}"
        );
        assert!(
            report["data"]["build_context"].is_null(),
            "invalid dependency cannot publish a reusable context: {case}"
        );
    }
}
