// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Isolated libFuzzer build and smoke checks.
use std::fs;
use std::path::Component;
use std::path::Path;

use serde_json::Value;
use serde_json::json;

use crate::build_context::BuildContext;
use crate::config::Config;
use crate::config::Fuzz;
use crate::install::copy_tree;
use crate::runner::Runner;
use crate::sanitize::compile_flags;
use crate::sanitize::link_flags;
use crate::sanitize::platform;
use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::missing_config;
pub(crate) fn run(
    root: &Path,
    context: &BuildContext,
    fuzz: &Fuzz,
    _cfg: &Config,
    runner: &mut Runner,
) -> Result<Value> {
    platform(context)?;
    let mut results = Vec::new();
    // Validate every requested target before executing any fuzzer.
    for requested in &fuzz.targets {
        if Path::new(&requested.target).components().count() != 1
            || Path::new(&requested.target)
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(config("unsafe fuzz target name"));
        }
        let target = context
            .targets
            .iter()
            .find(|target| target["name"] == requested.target && target["type"] == "EXECUTABLE")
            .ok_or_else(|| config(format!("fuzz target missing: {}", requested.target)))?;
        let compile = compile_flags(target);
        let link = link_flags(target);
        if compile.is_empty()
            || compile
                .iter()
                .any(|group| !group.contains("fuzzer") && !group.contains("fuzzer-no-link"))
            || !link.contains("fuzzer")
        {
            return Err(config(
                "every fuzz target needs libFuzzer compile instrumentation and the linked fuzzer runtime",
            ));
        }
        if !target["artifacts"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| item.as_str().is_some_and(|name| Path::new(name).is_file()))
        }) {
            return Err(config("fuzz target has no executable artifact"));
        }
    }
    for requested in &fuzz.targets {
        let target = context
            .targets
            .iter()
            .find(|target| target["name"] == requested.target)
            .ok_or_else(missing_config("fuzz target missing"))?;
        let executable = target["artifacts"]
            .as_array()
            .and_then(|items| items.iter().find_map(Value::as_str))
            .ok_or_else(missing_config("fuzz executable missing"))?;
        if fuzz.mode == "build-only" {
            results.push(json!({"target":requested.target,"mode":"build-only","executed":false}));
            continue;
        }
        let workspace = runner.work_root.join("fuzz").join(&requested.target);
        let corpus = workspace.join("corpus");
        let crashes = workspace.join("crashes");
        copy_tree(&root.join(&requested.corpus), &corpus)?;
        fs::create_dir_all(&crashes)?;
        let args = vec![
            corpus.to_string_lossy().into_owned(),
            format!("-max_total_time={}", requested.seconds),
            format!("-max_len={}", requested.max_len),
            format!("-timeout={}", requested.timeout_seconds),
            format!("-artifact_prefix={}/", crashes.display()),
        ];
        let reproduction = json!({"program":executable,"args":args});
        crate::report::write(&workspace.join("reproduction.json"), &reproduction)?;
        runner.run(
            executable,
            &args,
            root,
            requested
                .seconds
                .saturating_add(requested.timeout_seconds)
                .saturating_add(5),
            &[],
        )?;
        results.push(json!({"target":requested.target,"mode":"smoke","executed":true,"crash_directory":crashes,"reproduction":reproduction}));
    }
    Ok(json!({"targets":results}))
}
