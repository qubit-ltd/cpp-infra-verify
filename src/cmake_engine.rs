// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Preserve provenance of the engine's exact versioned platform files.
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;
use serde_json::json;

use crate::digest;
use crate::file_api;
use crate::verify_error::Result;
use crate::verify_error::config;

const NAMES: [&str; 5] = [
    "CMakeSystem.cmake",
    "CMakeCCompiler.cmake",
    "CMakeCXXCompiler.cmake",
    "CMakeASMCompiler.cmake",
    "CMakeRCCompiler.cmake",
];
fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    match value.get(name).and_then(Value::as_str) {
        Some(value) => Ok(value),
        None => Err(config(format!("CMake engine proof missing {name}"))),
    }
}
fn files(value: &Value) -> Result<&Vec<Value>> {
    match value.get("files").and_then(Value::as_array) {
        Some(value) => Ok(value),
        None => Err(config("CMake engine proof missing file list")),
    }
}
fn identity(version: &str) -> Value {
    json!({"version":version,"host_os":std::env::consts::OS,"host_arch":std::env::consts::ARCH})
}
pub(crate) fn owns(binary: &Path, version: &str, path: &Path) -> bool {
    path.parent() == Some(binary.join("CMakeFiles").join(version).as_path())
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| NAMES.contains(&name))
}
fn verify_groups(root: &Path, binary: &Path, groups: &[Value]) -> Result<()> {
    for group in groups {
        let version = text(&group["engine"], "version")?;
        if text(&group["engine"], "host_os")? != std::env::consts::OS
            || text(&group["engine"], "host_arch")? != std::env::consts::ARCH
        {
            return Err(config(
                "CMake host changed; select a fresh dedicated binaryDir for the new host",
            ));
        }
        for record in files(group)? {
            let path = PathBuf::from(text(record, "path")?);
            if !owns(binary, version, &path) || !path.starts_with(root) {
                return Err(config(
                    "CMake engine proof is outside the exact versioned platform-file namespace",
                ));
            }
            if !path.exists() {
                if fs::symlink_metadata(&path).is_ok() {
                    return Err(config("CMake engine artifact cannot be a dangling symlink"));
                }
                continue;
            }
            if path.canonicalize()? != path || !path.is_file() || digest::file(&path)? != text(record, "sha256")? {
                return Err(config(
                    "previous CMake engine artifact changed; restore its verified bytes or select a fresh binaryDir",
                ));
            }
        }
    }
    Ok(())
}
fn legacy_version(binary: &Path, state: &Value) -> Result<String> {
    let cache = binary.join("CMakeCache.txt");
    if state["cmake_cache_digest"] != digest::file(&cache)? {
        return Err(config(
            "legacy CMake owner proof cannot be reconstructed from a changed cache; use a fresh binaryDir",
        ));
    }
    let content = fs::read_to_string(cache)?;
    let mut parts = Vec::new();
    for key in [
        "CMAKE_CACHE_MAJOR_VERSION",
        "CMAKE_CACHE_MINOR_VERSION",
        "CMAKE_CACHE_PATCH_VERSION",
    ] {
        let mut selected = None;
        for line in content.lines() {
            if let Some((name, value)) = line.split_once('=')
                && name.split(':').next() == Some(key)
            {
                selected = Some(value.to_owned());
                break;
            }
        }
        match selected {
            Some(value) if value.bytes().all(|byte| byte.is_ascii_digit()) => parts.push(value),
            _ => return Err(config("legacy CMake version proof unavailable; use a fresh binaryDir")),
        }
    }
    Ok(parts.join("."))
}
fn legacy_host(records: &[Value]) -> Result<()> {
    let record = records.iter().find(|record| {
        record["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("/CMakeSystem.cmake"))
    });
    let Some(record) = record else {
        return Err(config("legacy CMake host proof unavailable; select a fresh binaryDir"));
    };
    let content = fs::read_to_string(text(record, "path")?)?;
    let read = |name: &str| -> Option<String> {
        let prefix = format!("set({name} \"");
        content.lines().find_map(|line| {
            line.trim()
                .strip_prefix(&prefix)
                .and_then(|value| value.strip_suffix("\")"))
                .map(str::to_owned)
        })
    };
    let expected_os = match std::env::consts::OS {
        "macos" => "darwin",
        value => value,
    };
    let Some(os) = read("CMAKE_HOST_SYSTEM_NAME") else {
        return Err(config("legacy CMake host name unavailable; select a fresh binaryDir"));
    };
    let Some(arch) = read("CMAKE_HOST_SYSTEM_PROCESSOR") else {
        return Err(config(
            "legacy CMake host architecture unavailable; select a fresh binaryDir",
        ));
    };
    let normalize = |value: &str| -> String {
        match value.to_ascii_lowercase().as_str() {
            "amd64" | "x86_64" => "x86_64".into(),
            "arm64" | "aarch64" => "aarch64".into(),
            "i386" | "i686" | "x86" => "x86".into(),
            value => value.to_owned(),
        }
    };
    if os.to_ascii_lowercase() != expected_os || normalize(&arch) != normalize(std::env::consts::ARCH) {
        return Err(config("CMake host changed; select a fresh dedicated binaryDir"));
    }
    Ok(())
}
pub(crate) fn previous(root: &Path, binary: &Path) -> Result<Vec<Value>> {
    let path = binary.join(".cpp-infra-build-state.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let state = file_api::json_file(&path)?;
    if state["schema"] != 1 || state["project"] != root.to_string_lossy().as_ref() {
        return Err(config("CMake engine state belongs to a different project"));
    }
    let groups = if let Some(value) = state.get("cmake_engine_proofs") {
        match value.as_array() {
            Some(value) => value.clone(),
            None => return Err(config("CMake engine proof collection must be an array")),
        }
    } else {
        let records: Vec<Value> = state["configuration_inputs"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter(|value| value["kind"] == "cmake-generated")
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if records.is_empty() {
            Vec::new()
        } else {
            let version = legacy_version(binary, &state)?;
            legacy_host(&records)?;
            vec![json!({"engine":identity(&version),"files":records})]
        }
    };
    verify_groups(root, binary, &groups)?;
    Ok(groups)
}
pub(crate) fn check_current(root: &Path, binary: &Path, version: &str, groups: &[Value]) -> Result<()> {
    if version.is_empty() || version.contains(['/', '\\']) || version == "." || version == ".." {
        return Err(config("invalid CMake engine version"));
    }
    verify_groups(root, binary, groups)?;
    for name in NAMES {
        let path = binary.join("CMakeFiles").join(version).join(name);
        if !path.exists() {
            if fs::symlink_metadata(&path).is_ok() {
                return Err(config("CMake engine artifact cannot be a dangling symlink"));
            }
            continue;
        }
        let mut proven = false;
        for group in groups {
            for record in files(group)? {
                if record["path"] == path.to_string_lossy().as_ref() && record["sha256"] == digest::file(&path)? {
                    proven = true;
                }
            }
        }
        if !proven {
            return Err(config(
                "existing versioned CMake platform file has no prior generation proof; select a fresh binaryDir",
            ));
        }
    }
    Ok(())
}
pub(crate) fn capture(binary: &Path, version: &str, groups: &[Value]) -> Result<Vec<Value>> {
    let mut records = Vec::new();
    for name in NAMES {
        let path = binary.join("CMakeFiles").join(version).join(name);
        if !path.exists() {
            continue;
        }
        if path.canonicalize()? != path || !path.is_file() {
            return Err(config("CMake produced a platform-file symlink or non-file"));
        }
        records.push(json!({"path":path,"kind":"cmake-generated","sha256":digest::file(&path)?}));
    }
    let mut groups: Vec<Value> = groups
        .iter()
        .filter(|group| group["engine"]["version"] != version)
        .cloned()
        .collect();
    groups.push(json!({"engine":identity(version),"files":records}));
    groups.sort_by(|left, right| {
        left["engine"]["version"]
            .as_str()
            .cmp(&right["engine"]["version"].as_str())
    });
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use crate::cmake_engine;

    #[test]
    fn test_new_engine_version_uses_a_new_namespace_and_preserves_verified_history() {
        let temp = tempfile::tempdir().expect("engine fixture");
        let root = temp.path();
        let binary = root.join("build/infra/debug");
        let old = binary.join("CMakeFiles/3.25.0/CMakeSystem.cmake");
        fs::create_dir_all(old.parent().expect("parent")).expect("old directory");
        fs::write(&old, "set(CMAKE_SYSTEM_LOADED 1)\n").expect("owned historical output");
        let groups = cmake_engine::capture(&binary, "3.25.0", &[]).expect("old generation evidence");
        cmake_engine::check_current(root, &binary, "3.28.3", &groups).expect("new version has no unproven output");
        let new = binary.join("CMakeFiles/3.28.3/CMakeSystem.cmake");
        fs::create_dir_all(new.parent().expect("parent")).expect("new directory");
        fs::write(&new, "set(CMAKE_SYSTEM_LOADED 1)\n").expect("new output");
        let groups = cmake_engine::capture(&binary, "3.28.3", &groups).expect("new generation evidence");
        assert_eq!(groups.len(), 2);
        assert!(old.is_file());
        cmake_engine::check_current(root, &binary, "3.25.0", &groups).expect("verified downgrade namespace");
        fs::write(&old, "set(USER_POLICY ON)\n").expect("unexpected edit");
        assert!(cmake_engine::check_current(root, &binary, "3.25.0", &groups).is_err());
        assert_eq!(
            fs::read_to_string(&old).expect("preserved user data"),
            "set(USER_POLICY ON)\n"
        );
    }
    #[test]
    fn test_preexisting_unproven_platform_file_and_unsafe_versions_are_rejected() {
        let temp = tempfile::tempdir().expect("engine fixture");
        let binary = temp.path().join("build/infra/debug");
        let path = binary.join("CMakeFiles/3.28.3/CMakeSystem.cmake");
        fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        fs::write(&path, "set(USER_POLICY ON)\n").expect("unproven policy");
        assert!(cmake_engine::check_current(temp.path(), &binary, "3.28.3", &[]).is_err());
        assert!(cmake_engine::check_current(temp.path(), &binary, "../escape", &[]).is_err());
        assert!(path.is_file());
    }
    #[test]
    fn test_malformed_engine_proofs_fail_without_adopting_their_files() {
        let temp = tempfile::tempdir().expect("engine fixture");
        let binary = temp.path().join("build/infra/debug");
        fs::create_dir_all(&binary).expect("binary");
        let state = binary.join(".cpp-infra-build-state.json");
        for proofs in [
            json!("wrong-type"),
            json!([{}]),
            json!([{"engine":{"version":"3.28.3"},"files":[]}]),
            json!([{"engine":{"version":"3.28.3","host_os":std::env::consts::OS,"host_arch":std::env::consts::ARCH}}]),
        ] {
            fs::write(
                &state,
                json!({"schema":1,"project":temp.path(),"cmake_engine_proofs":proofs}).to_string(),
            )
            .expect("state");
            assert!(cmake_engine::previous(temp.path(), &binary).is_err());
        }
    }
}
