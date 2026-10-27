// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Relocated installation consumer verification.
use std::fs;
use std::path::Path;

use serde_json::Value;
use serde_json::json;
use walkdir::WalkDir;

use crate::build_context::BuildContext;
use crate::config::Config;
use crate::config::Install;
use crate::headers::cmake_quote;
use crate::runner::Runner;
use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::gate;
use crate::verify_error::missing_config;
use crate::verify_error::protocol_error;
pub(crate) fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    let canonical = from.canonicalize()?;
    for entry in WalkDir::new(from).follow_links(false) {
        let entry = entry.map_err(protocol_error)?;
        let relative = entry.path().strip_prefix(from).map_err(protocol_error)?;
        let destination = to.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(destination)?;
        } else {
            if !entry.path().canonicalize()?.starts_with(&canonical) {
                return Err(config("copied tree contains escaping symlink"));
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}
pub(crate) fn run(
    root: &Path,
    context: &BuildContext,
    install: &Install,
    cfg: &Config,
    runner: &mut Runner,
) -> Result<Value> {
    let prefix_a = runner.work_root.join("install/prefix-A");
    let prefix_b = runner.work_root.join("install/prefix-B");
    let mut args = vec![
        "--install".into(),
        context.binary_dir.to_string_lossy().into_owned(),
        "--prefix".into(),
        prefix_a.to_string_lossy().into_owned(),
    ];
    if let Some(configuration) = &context.configuration {
        args.extend(["--config".into(), configuration.clone()]);
    }
    runner.run("cmake", &args, root, cfg.build_timeout_seconds, &[])?;
    if fs::symlink_metadata(&prefix_a)?.file_type().is_symlink() {
        return Err(config("install prefix-A must be the owned staging directory"));
    }
    copy_tree(&prefix_a, &prefix_b)?;
    // A is owned by this invocation; it must be unavailable during relocation
    // checks.
    fs::remove_dir_all(&prefix_a)?;
    for header in &install.headers {
        if !prefix_b.join(header).is_file() && !prefix_b.join("include").join(header).is_file() {
            return Err(gate(format!("installed header missing: {header}")));
        }
    }
    let env = vec![
        (
            "LD_LIBRARY_PATH".into(),
            format!("{}/lib:{}/lib64", prefix_b.display(), prefix_b.display()),
        ),
        ("DYLD_LIBRARY_PATH".into(), format!("{}/lib", prefix_b.display())),
    ];
    if !install.executables.is_empty() {
        for executable in &install.executables {
            let binary = prefix_b.join(executable);
            if !binary.is_file() {
                return Err(gate(format!("installed executable missing: {executable}")));
            }
            if install.run_consumer {
                run_consumer(root, context, install, &binary, &install.smoke_args, cfg, runner, &env)?;
            }
        }
        return Ok(json!({"prefix":prefix_b,"mode":if install.run_consumer{"executed"}else{"build-only"}}));
    }
    let package = install
        .package
        .as_ref()
        .ok_or_else(missing_config("install package missing"))?;
    if !package
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(config("invalid CMake package name"));
    }
    let names = [
        format!("{package}Config.cmake"),
        format!("{}-config.cmake", package.to_ascii_lowercase()),
    ];
    let mut configs = Vec::new();
    for entry in WalkDir::new(&prefix_b) {
        let entry = entry.map_err(protocol_error)?;
        if entry.file_type().is_file() && names.iter().any(|name| entry.file_name() == name.as_str()) {
            configs.push(entry.into_path());
        }
    }
    if configs.len() != 1 {
        return Err(gate(format!(
            "expected one relocated {package} config, found {}",
            configs.len()
        )));
    }
    for entry in WalkDir::new(&prefix_b) {
        let entry = entry.map_err(protocol_error)?;
        if entry.path().extension().is_some_and(|ext| ext == "cmake") {
            let content = fs::read_to_string(entry.path())?
                .replace(&prefix_b.to_string_lossy().to_string(), "<relocated-prefix>");
            for forbidden in [root, &context.binary_dir, &prefix_a] {
                if content.contains(&forbidden.to_string_lossy().to_string()) {
                    return Err(gate(format!(
                        "installed CMake file leaks forbidden path: {}",
                        entry.path().display()
                    )));
                }
            }
        }
    }
    let source = runner.work_root.join("install/consumer");
    let binary = runner.work_root.join("install/consumer-build");
    fs::create_dir_all(&source)?;
    let consumer = install
        .consumer_source
        .as_ref()
        .ok_or_else(missing_config("consumer_source required"))?;
    fs::copy(root.join(consumer), source.join("main.cpp"))?;
    let config_dir = configs[0].parent().ok_or_else(missing_config("config parent"))?;
    let mut cmake = format!(
        "cmake_minimum_required(VERSION 3.25)\nproject(InfraConsumer LANGUAGES CXX)\nset(CMAKE_FIND_USE_PACKAGE_REGISTRY OFF)\nset(CMAKE_FIND_USE_SYSTEM_PACKAGE_REGISTRY OFF)\nset({package}_DIR {} CACHE PATH \"\" FORCE)\nfind_package({package} CONFIG REQUIRED NO_DEFAULT_PATH PATHS {})\nadd_executable(infra_consumer main.cpp)\n",
        cmake_quote(&config_dir.to_string_lossy()),
        cmake_quote(&config_dir.to_string_lossy())
    );
    for target in &install.targets {
        if target.contains(['\n', ';', '"']) {
            return Err(config("unsafe install target name"));
        }
        cmake.push_str(&format!("if(NOT TARGET {target})\n message(FATAL_ERROR \"Missing exported target {target}\")\nendif()\ntarget_link_libraries(infra_consumer PRIVATE {target})\nforeach(_property INTERFACE_INCLUDE_DIRECTORIES INTERFACE_LINK_DIRECTORIES IMPORTED_LOCATION IMPORTED_IMPLIB IMPORTED_LOCATION_DEBUG IMPORTED_LOCATION_RELEASE INTERFACE_LINK_LIBRARIES)\n get_target_property(_value {target} ${{_property}})\n"));
        cmake.push_str(&format!(
            " string(REPLACE {} \"<relocated-prefix>\" _value \"${{_value}}\")\n",
            cmake_quote(&prefix_b.to_string_lossy())
        ));
        for forbidden in [root, &context.binary_dir, &prefix_a] {
            cmake.push_str(&format!(" string(FIND \"${{_value}}\" {} _found)\n if(NOT _found EQUAL -1)\n  message(FATAL_ERROR \"exported target leaks source/build/original install path\")\n endif()\n",cmake_quote(&forbidden.to_string_lossy())));
        }
        cmake.push_str("endforeach()\n");
    }
    fs::write(source.join("CMakeLists.txt"), cmake)?;
    let mut configure = vec![
        "-S".into(),
        source.to_string_lossy().into_owned(),
        "-B".into(),
        binary.to_string_lossy().into_owned(),
        "-G".into(),
        "Ninja".into(),
        format!("-DCMAKE_PREFIX_PATH={}", prefix_b.display()),
        format!(
            "-DCMAKE_CXX_COMPILER={}",
            context.compiler["path"]
                .as_str()
                .ok_or_else(missing_config("compiler path"))?
        ),
    ];
    configure.extend(crate::file_api::consumer_arguments(context)?);
    if let Some(configuration) = &context.configuration {
        configure.push(format!("-DCMAKE_BUILD_TYPE={configuration}"));
    }
    runner.run("cmake", &configure, root, cfg.configure_timeout_seconds, &[])?;
    runner.run(
        "cmake",
        &[
            "--build".into(),
            binary.to_string_lossy().into_owned(),
            "--parallel".into(),
            cfg.jobs.to_string(),
        ],
        root,
        cfg.build_timeout_seconds,
        &[],
    )?;
    if install.run_consumer {
        run_consumer(
            root,
            context,
            install,
            &binary.join("infra_consumer"),
            &[],
            cfg,
            runner,
            &env,
        )?;
    }
    Ok(json!({"prefix":prefix_b,"package":package,"mode":if install.run_consumer{"executed"}else{"build-only"}}))
}
#[allow(clippy::too_many_arguments)]
fn run_consumer(
    root: &Path,
    context: &BuildContext,
    install: &Install,
    binary: &Path,
    args: &[String],
    cfg: &Config,
    runner: &mut Runner,
    env: &[(String, String)],
) -> Result<()> {
    let system = context.binary_dir.join("CMakeFiles");
    let mut cross = false;
    for entry in WalkDir::new(system).max_depth(3) {
        let entry = entry.map_err(protocol_error)?;
        if entry.file_name() == "CMakeSystem.cmake" {
            let value = fs::read_to_string(entry.path())?;
            cross |= value.contains("CMAKE_CROSSCOMPILING \"TRUE\"") || value.contains("CMAKE_CROSSCOMPILING TRUE");
        }
    }
    if cross && install.emulator.is_empty() {
        return Err(config(
            "cross-compiled consumer execution requires an explicit emulator or run_consumer=false",
        ));
    }
    if let Some((program, emulator_args)) = install.emulator.split_first() {
        let mut execution = emulator_args.to_vec();
        execution.push(binary.to_string_lossy().into_owned());
        execution.extend_from_slice(args);
        runner.run(program, &execution, root, cfg.test_timeout_seconds, env)?;
    } else {
        runner.run(&binary.to_string_lossy(), args, root, cfg.test_timeout_seconds, env)?;
    }
    Ok(())
}
