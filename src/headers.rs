// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Compile one translation unit per selected public header using target usage
//! requirements.
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;
use serde_json::json;
use walkdir::WalkDir;

use crate::build_context::BuildContext;
use crate::config::Config;
use crate::config::Headers;
use crate::runner::Runner;
use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::gate;
use crate::verify_error::protocol_error;
pub(crate) fn cmake_quote(path: &str) -> String {
    format!(
        "\"{}\"",
        path.replace('\\', "/").replace('"', "\\\"").replace(';', "\\;")
    )
}
pub(crate) fn run(
    root: &Path,
    context: &BuildContext,
    headers: &Headers,
    cfg: &Config,
    runner: &mut Runner,
) -> Result<Value> {
    let source = runner.work_root.join("headers/source");
    let binary = runner.work_root.join("headers/build");
    fs::create_dir_all(&source)?;
    let mut selected = BTreeSet::new();
    for directory in &headers.roots {
        for entry in WalkDir::new(root.join(directory)).follow_links(false) {
            let entry = entry.map_err(protocol_error)?;
            if entry.file_type().is_symlink() {
                return Err(config("symlinks in public headers are not supported"));
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !matches!(
                path.extension().and_then(|p| p.to_str()),
                Some("h" | "hh" | "hpp" | "hxx" | "tcc" | "ipp")
            ) {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .map_err(protocol_error)?
                .to_string_lossy()
                .replace('\\', "/");
            if headers.exclude.contains(&relative) {
                continue;
            }
            selected.insert(relative);
        }
    }
    for host in headers.exceptions.values() {
        selected.insert(host.clone());
    }
    if selected.is_empty() {
        return Err(gate("headers suite selected no public headers"));
    }
    let mut cmake = String::from("add_library(infra_headers OBJECT\n");
    for (index, header) in selected.iter().enumerate() {
        let tu = source.join(format!("header_{index}.cpp"));
        fs::write(
            &tu,
            format!("#include {}\n", cmake_quote(&root.join(header).to_string_lossy())),
        )?;
        cmake.push_str(&format!("  {}\n", cmake_quote(&tu.to_string_lossy())));
    }
    cmake.push_str(")\n");
    let targets: Vec<String> = if headers.targets.is_empty() {
        context
            .targets
            .iter()
            .filter(|target| {
                [
                    "STATIC_LIBRARY",
                    "SHARED_LIBRARY",
                    "INTERFACE_LIBRARY",
                    "OBJECT_LIBRARY",
                    "EXECUTABLE",
                ]
                .contains(&target["type"].as_str().unwrap_or(""))
            })
            .filter_map(|target| target["name"].as_str().map(str::to_owned))
            .collect()
    } else {
        headers.targets.clone()
    };
    if targets.is_empty() {
        return Err(config("headers require a project target with usage requirements"));
    }
    for target in targets {
        if target.contains(['\n', ';', '"']) {
            return Err(config("unsafe header target name"));
        }
        cmake.push_str(&format!("if(NOT TARGET {target})\n message(FATAL_ERROR \"Missing headers target {target}\")\nendif()\nget_target_property(_kind {target} TYPE)\nif(_kind STREQUAL \"EXECUTABLE\")\n target_include_directories(infra_headers PRIVATE $<TARGET_PROPERTY:{target},INCLUDE_DIRECTORIES>)\n target_compile_definitions(infra_headers PRIVATE $<TARGET_PROPERTY:{target},COMPILE_DEFINITIONS>)\n target_compile_options(infra_headers PRIVATE $<TARGET_PROPERTY:{target},COMPILE_OPTIONS>)\n target_compile_features(infra_headers PRIVATE $<TARGET_PROPERTY:{target},COMPILE_FEATURES>)\nelse()\n target_link_libraries(infra_headers PRIVATE {target})\nendif()\n"));
    }
    let checks = source.join("headers-check.cmake");
    fs::write(&checks, cmake)?;
    let hook = source.join("headers-profile-hook.cmake");
    fs::write(
        &hook,
        format!(
            "cmake_language(DEFER DIRECTORY \"${{CMAKE_SOURCE_DIR}}\" CALL include {})\n",
            cmake_quote(&checks.to_string_lossy())
        ),
    )?;
    let mut includes = crate::file_api::cache_value(context, "CMAKE_PROJECT_TOP_LEVEL_INCLUDES")?.unwrap_or_default();
    if !includes.is_empty() {
        includes.push(';');
    }
    includes.push_str(&hook.to_string_lossy());
    let mut args = vec![
        "--preset".into(),
        context.configure_preset.clone(),
        "-B".into(),
        binary.to_string_lossy().into_owned(),
    ];
    args.extend(crate::file_api::header_arguments(context)?);
    args.push(format!("-DCMAKE_PROJECT_TOP_LEVEL_INCLUDES:STRING={includes}"));
    runner.run("cmake", &args, root, cfg.configure_timeout_seconds, &[])?;
    let mut build = vec![
        "--build".into(),
        binary.to_string_lossy().into_owned(),
        "--target".into(),
        "infra_headers".into(),
        "--parallel".into(),
        cfg.jobs.to_string(),
    ];
    if let Some(configuration) = &context.configuration {
        build.extend(["--config".into(), configuration.clone()]);
    }
    runner.run("cmake", &build, root, cfg.build_timeout_seconds, &[])?;
    Ok(json!({"headers":selected,"translation_units":source,"checked":selected.len()}))
}
