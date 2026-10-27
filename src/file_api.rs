// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Read CMake-owned File API replies, never parse console output for targets.
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;
use serde_json::json;

use crate::build_context::BuildContext;
use crate::config::Profile;
use crate::digest;
use crate::presets::Presets;
use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::missing_config;
use crate::verify_error::protocol_error;
pub(crate) fn json_file(path: &Path) -> Result<Value> {
    serde_json::from_slice(&fs::read(path)?).map_err(|error| config(format!("{}: {error}", path.display())))
}
pub(crate) fn query(binary: &Path) -> Result<()> {
    let directory = binary.join(".cmake/api/v1/query/client-cpp-infra-verify");
    fs::create_dir_all(&directory)?;
    fs::write(directory.join("query.json"),serde_json::to_vec(&json!({"requests":[{"kind":"codemodel","version":2},{"kind":"cache","version":2},{"kind":"toolchains","version":1},{"kind":"cmakeFiles","version":1}]})).map_err(protocol_error)?)?;
    Ok(())
}
fn reply(directory: &Path, index: &Value, kind: &str) -> Result<Value> {
    let reference = index["objects"]
        .as_array()
        .and_then(|objects| objects.iter().find(|object| object["kind"] == kind))
        .and_then(|value| value["jsonFile"].as_str())
        .ok_or_else(|| config(format!("File API has no {kind} reply")))?;
    if Path::new(reference).components().count() != 1 {
        return Err(config("unsafe File API reference"));
    }
    json_file(&directory.join(reference))
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn read(
    root: &Path,
    name: &str,
    profile: &Profile,
    presets: &Presets,
    source_digest: &str,
    config_digest: &str,
) -> Result<BuildContext> {
    let directory = presets.binary_dir.join(".cmake/api/v1/reply");
    let mut indexes: Vec<PathBuf> = fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<_>>()?;
    indexes.retain(|path| {
        path.file_name().is_some_and(|name| {
            name.to_string_lossy().starts_with("index-") && name.to_string_lossy().ends_with(".json")
        })
    });
    indexes.sort();
    let path = indexes.last().ok_or_else(missing_config("no CMake File API index"))?;
    let index = json_file(path)?;
    let model = reply(&directory, &index, "codemodel")?;
    let tools = reply(&directory, &index, "toolchains")?;
    let cache = reply(&directory, &index, "cache")?;
    if Path::new(
        model["paths"]["source"]
            .as_str()
            .ok_or_else(missing_config("File API missing source"))?,
    )
    .canonicalize()?
        != root
    {
        return Err(config("CMake File API source root mismatch"));
    }
    if Path::new(
        model["paths"]["build"]
            .as_str()
            .ok_or_else(missing_config("File API missing build"))?,
    )
    .canonicalize()?
        != presets.binary_dir.canonicalize()?
    {
        return Err(config("CMake File API binary directory mismatch"));
    }
    let generator = index["cmake"]["generator"]["name"]
        .as_str()
        .ok_or_else(missing_config("File API missing generator"))?;
    if generator != presets.generator {
        return Err(config("actual generator differs from committed preset"));
    }
    let mut compiler = tools["toolchains"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["language"] == "CXX"))
        .ok_or_else(missing_config("CXX toolchain required"))?["compiler"]
        .clone();
    for key in ["id", "path", "version"] {
        if compiler[key].as_str().is_none() {
            return Err(config(format!("CXX compiler missing {key}")));
        }
    }
    if let Some(declared) = presets.configure["cacheVariables"]["CMAKE_CXX_COMPILER"].as_str() {
        let actual = compiler["path"].as_str().ok_or_else(missing_config("compiler path"))?;
        let declared = declared.replace("${sourceDir}", &root.to_string_lossy());
        if Path::new(&declared).is_absolute() && fs::canonicalize(declared)? != fs::canonicalize(actual)? {
            return Err(config("CXX compiler differs from committed preset"));
        }
    }
    let configs = model["configurations"]
        .as_array()
        .ok_or_else(missing_config("File API missing configurations"))?;
    let selected = if let Some(name) = &presets.configuration {
        configs
            .iter()
            .find(|item| item["name"] == *name)
            .ok_or_else(missing_config("configured build configuration not found"))?
    } else {
        if configs.len() != 1 {
            return Err(config("ambiguous File API configuration"));
        }
        &configs[0]
    };
    let mut targets = Vec::new();
    for target in selected["targets"]
        .as_array()
        .ok_or_else(missing_config("File API missing targets"))?
    {
        let reference = target["jsonFile"]
            .as_str()
            .ok_or_else(missing_config("target missing jsonFile"))?;
        if Path::new(reference).components().count() != 1 {
            return Err(config("unsafe target reply path"));
        }
        let details = json_file(&directory.join(reference))?;
        let artifacts = details["artifacts"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["path"].as_str())
                    .map(|path| presets.binary_dir.join(path))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let sources = details["sources"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["path"].as_str())
                    .map(|path| root.join(path))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        targets.push(json!({"name":details["name"],"type":details["type"],"artifacts":artifacts,"sources":sources,"compile_groups":details["compileGroups"],"link_fragments":details["link"]["commandFragments"]}));
    }
    if !targets.iter().any(|target| {
        target["compile_groups"]
            .as_array()
            .is_some_and(|groups| groups.iter().any(|group| group["language"] == "CXX"))
    }) {
        return Err(config(
            "project has no buildable targets; header-only needs a compilation fixture",
        ));
    }
    let cache_value = |key: &str| -> Option<String> {
        cache["entries"].as_array()?.iter().find(|entry| entry["name"] == key)?["value"]
            .as_str()
            .map(str::to_owned)
    };
    let mut standards = std::collections::BTreeSet::new();
    for target in &targets {
        if let Some(groups) = target["compile_groups"].as_array() {
            for group in groups {
                if group["language"] == "CXX"
                    && let Some(standard) = group["languageStandard"]["standard"].as_str()
                {
                    standards.insert(standard.to_owned());
                }
            }
        }
    }
    compiler["cppstd"] = json!(if standards.len() == 1 {
        standards.first().cloned()
    } else {
        cache_value("CMAKE_CXX_STANDARD")
    });
    compiler["runtime"] = json!(cache_value("CMAKE_MSVC_RUNTIME_LIBRARY"));
    compiler["build_type"] = selected["name"].clone();
    let compile_commands = presets.binary_dir.join("compile_commands.json");
    if (name == "analysis" || name == "coverage") && !compile_commands.is_file() {
        return Err(config("analysis/coverage profile must export compile_commands.json"));
    }
    let cmake_cache_digest = digest::file(&presets.binary_dir.join("CMakeCache.txt"))?;
    let fingerprint=digest::bytes(serde_json::to_string(&json!({"source":source_digest,"config":config_digest,"profile":name,"compiler":compiler,"generator":generator,"configuration":presets.configuration,"cmake_cache_digest":cmake_cache_digest})).map_err(protocol_error)?.as_bytes());
    Ok(BuildContext {
        schema: 1,
        project: root.to_path_buf(),
        profile: name.into(),
        binary_dir: presets.binary_dir.clone(),
        configure_preset: profile.configure.clone(),
        build_preset: profile.build.clone(),
        test_preset: profile.test.clone(),
        configuration: presets.configuration.clone(),
        source_digest: source_digest.into(),
        source_excluded_paths: Vec::new(),
        config_digest: config_digest.into(),
        fingerprint,
        compiler,
        generator: generator.into(),
        compile_commands: compile_commands.is_file().then_some(compile_commands),
        targets,
        file_api_index: path.clone(),
        dependency_context: None,
        configuration_inputs: Vec::new(),
        cmake_cache_digest,
    })
}

pub(crate) fn consumer_arguments(context: &BuildContext) -> Result<Vec<String>> {
    let directory = context
        .file_api_index
        .parent()
        .ok_or_else(missing_config("File API index parent"))?;
    let index = json_file(&context.file_api_index)?;
    let cache = reply(directory, &index, "cache")?;
    let keys = [
        "CMAKE_TOOLCHAIN_FILE",
        "CMAKE_SYSROOT",
        "CMAKE_SYSTEM_NAME",
        "CMAKE_SYSTEM_PROCESSOR",
        "CMAKE_CXX_COMPILER_TARGET",
        "CMAKE_CXX_COMPILER_EXTERNAL_TOOLCHAIN",
        "CMAKE_OSX_SYSROOT",
        "CMAKE_OSX_ARCHITECTURES",
        "CMAKE_MSVC_RUNTIME_LIBRARY",
        "CMAKE_BUILD_TYPE",
    ];
    let mut args = Vec::new();
    for entry in cache["entries"]
        .as_array()
        .ok_or_else(missing_config("File API cache entries missing"))?
    {
        let Some(name) = entry["name"].as_str() else {
            continue;
        };
        if keys.contains(&name) {
            let value = entry["value"]
                .as_str()
                .ok_or_else(missing_config("File API cache value is not a string"))?;
            args.push(format!("-D{name}={value}"));
        }
    }
    Ok(args)
}

pub(crate) fn cache_value(context: &BuildContext, name: &str) -> Result<Option<String>> {
    let directory = context
        .file_api_index
        .parent()
        .ok_or_else(missing_config("File API index parent"))?;
    let cache = reply(directory, &json_file(&context.file_api_index)?, "cache")?;
    Ok(cache["entries"]
        .as_array()
        .ok_or_else(missing_config("File API cache entries missing"))?
        .iter()
        .find(|entry| entry["name"] == name)
        .and_then(|entry| entry["value"].as_str())
        .map(str::to_owned))
}
pub(crate) fn header_arguments(context: &BuildContext) -> Result<Vec<String>> {
    let directory = context
        .file_api_index
        .parent()
        .ok_or_else(missing_config("File API index parent"))?;
    let cache = reply(directory, &json_file(&context.file_api_index)?, "cache")?;
    let mut arguments = Vec::new();
    for entry in cache["entries"]
        .as_array()
        .ok_or_else(missing_config("File API cache entries missing"))?
    {
        if matches!(entry["type"].as_str(), Some("BOOL" | "STRING" | "PATH" | "FILEPATH")) {
            let name = entry["name"]
                .as_str()
                .ok_or_else(missing_config("cache entry name missing"))?;
            let kind = entry["type"]
                .as_str()
                .ok_or_else(missing_config("cache entry type missing"))?;
            let value = entry["value"]
                .as_str()
                .ok_or_else(missing_config("cache entry value missing"))?;
            arguments.push(format!("-D{name}:{kind}={value}"));
        }
    }
    Ok(arguments)
}

pub(crate) fn configuration_files(context: &BuildContext) -> Result<(PathBuf, String, Vec<Value>)> {
    let directory = context
        .file_api_index
        .parent()
        .ok_or_else(missing_config("File API index parent"))?;
    let index = json_file(&context.file_api_index)?;
    let root = PathBuf::from(
        index["cmake"]["paths"]["root"]
            .as_str()
            .ok_or_else(missing_config("CMake installation root missing"))?,
    )
    .canonicalize()?;
    let input = reply(directory, &index, "cmakeFiles")?;
    let files = input["inputs"]
        .as_array()
        .ok_or_else(missing_config("CMake File API configuration input list missing"))?
        .clone();
    let version = index["cmake"]["version"]["string"]
        .as_str()
        .ok_or_else(missing_config("CMake engine version missing"))?
        .to_owned();
    Ok((root, version, files))
}
