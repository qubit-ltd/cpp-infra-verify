// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Verify declared sanitizer instrumentation and propagate real runtime
//! failures.
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::build_context::BuildContext;
use crate::config::Config;
use crate::config::Sanitize;
use crate::ctest;
use crate::runner::Runner;
use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::gate;
fn flags(fragments: &Value) -> BTreeSet<String> {
    let mut enabled = BTreeSet::new();
    if let Some(items) = fragments.as_array() {
        for item in items {
            if let Some(fragment) = item["fragment"].as_str() {
                for token in fragment.split_whitespace() {
                    let token = token.trim_matches(['\"', '\'']);
                    if let Some(values) = token.strip_prefix("-fsanitize=") {
                        enabled.extend(values.split(',').map(str::to_owned));
                    } else if let Some(values) = token.strip_prefix("-fno-sanitize=") {
                        for value in values.split(',') {
                            if value == "all" {
                                enabled.clear();
                            } else {
                                enabled.remove(value);
                            }
                        }
                    }
                }
            }
        }
    }
    enabled
}
pub(crate) fn compile_flags(target: &Value) -> Vec<BTreeSet<String>> {
    target["compile_groups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .map(|group| flags(&group["compileCommandFragments"]))
                .collect()
        })
        .unwrap_or_default()
}
pub(crate) fn link_flags(target: &Value) -> BTreeSet<String> {
    flags(&target["link_fragments"])
}
pub(crate) fn platform(context: &BuildContext) -> Result<()> {
    if std::env::consts::OS != "linux" || context.compiler["id"] != "Clang" || context.generator != "Ninja" {
        return Err(config("sanitize/fuzz requires Linux Clang and Ninja"));
    }
    Ok(())
}
pub(crate) fn run(
    root: &Path,
    context: &BuildContext,
    sanitize: &Sanitize,
    cfg: &Config,
    runner: &mut Runner,
) -> Result<Value> {
    platform(context)?;
    let selected: BTreeSet<&str> = sanitize.sanitizers.iter().map(String::as_str).collect();
    if !(selected == BTreeSet::from(["address", "undefined"]) || selected == BTreeSet::from(["thread"]))
        || selected.len() != sanitize.sanitizers.len()
    {
        return Err(config(
            "supported sanitizer sets: address+undefined, or thread separately",
        ));
    }
    for target in &context.targets {
        if !["EXECUTABLE", "STATIC_LIBRARY", "SHARED_LIBRARY", "OBJECT_LIBRARY"]
            .contains(&target["type"].as_str().unwrap_or(""))
        {
            continue;
        }
        let compile = compile_flags(target);
        let link = link_flags(target);
        for group in compile.iter().chain(std::iter::once(&link)) {
            if selected.contains("thread") && group.contains("address")
                || selected.contains("address") && group.contains("thread")
            {
                return Err(config("thread and address sanitizer instrumentation cannot be mixed"));
            }
        }
        if compile.is_empty()
            || compile
                .iter()
                .any(|group| selected.iter().any(|expected| !group.contains(*expected)))
        {
            return Err(config(format!(
                "target {} lacks required sanitizer compile instrumentation",
                target["name"]
            )));
        }
        if (target["type"] == "EXECUTABLE" || target["type"] == "SHARED_LIBRARY")
            && selected.iter().any(|expected| !link.contains(*expected))
        {
            return Err(config("sanitizer link instrumentation missing"));
        }
    }
    let env = vec![
        ("ASAN_OPTIONS".into(), "halt_on_error=1:detect_leaks=1".into()),
        ("UBSAN_OPTIONS".into(), "halt_on_error=1:print_stacktrace=1".into()),
        ("TSAN_OPTIONS".into(), "halt_on_error=1:exitcode=66".into()),
    ];
    let result = ctest::run(root, context, cfg, runner, &env)?;
    let log = context.binary_dir.join("Testing/Temporary/LastTest.log");
    if log.is_file() {
        let output = fs::read_to_string(log)?;
        if [
            "ERROR: AddressSanitizer",
            "runtime error:",
            "WARNING: ThreadSanitizer",
            "UndefinedBehaviorSanitizer",
        ]
        .iter()
        .any(|text| output.contains(text))
        {
            return Err(gate("sanitizer report appeared in test output"));
        }
    }
    Ok(result)
}
