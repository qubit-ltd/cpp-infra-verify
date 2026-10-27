// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! CTest selection and real execution gate.
use std::fs;
use std::path::Path;

use serde_json::Value;
use serde_json::json;

use crate::build_context::BuildContext;
use crate::config::Config;
use crate::runner::Runner;
use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::gate;
use crate::verify_error::missing_config;
pub(crate) fn run(
    root: &Path,
    context: &BuildContext,
    cfg: &Config,
    runner: &mut Runner,
    env: &[(String, String)],
) -> Result<Value> {
    let preset = context
        .test_preset
        .as_ref()
        .ok_or_else(missing_config("test suite requires a committed test preset"))?;
    let selection = runner.run(
        "ctest",
        &["--preset".into(), preset.clone(), "--show-only=json-v1".into()],
        root,
        cfg.test_timeout_seconds,
        env,
    )?;
    let value: Value = serde_json::from_str(&selection).map_err(|e| config(format!("CTest selection: {e}")))?;
    let tests = value["tests"]
        .as_array()
        .ok_or_else(missing_config("CTest selection has no tests array"))?;
    let enabled = tests
        .iter()
        .filter(|test| {
            !test["properties"].as_array().is_some_and(|properties| {
                properties.iter().any(|property| {
                    property["name"] == "DISABLED" && (property["value"] == true || property["value"] == "TRUE")
                })
            })
        })
        .count();
    if enabled == 0 {
        return Err(gate("CTest selected zero runnable tests"));
    }
    let junit = runner.work_root.join("ctest-junit.xml");
    runner.run(
        "ctest",
        &[
            "--preset".into(),
            preset.clone(),
            "--output-on-failure".into(),
            "--no-tests=error".into(),
            "--output-junit".into(),
            junit.to_string_lossy().into_owned(),
        ],
        root,
        cfg.test_timeout_seconds,
        env,
    )?;
    let xml = fs::read_to_string(&junit)?;
    let document = roxmltree::Document::parse(&xml).map_err(|e| config(format!("CTest JUnit: {e}")))?;
    let cases: Vec<_> = document
        .descendants()
        .filter(|node| node.has_tag_name("testcase"))
        .collect();
    let skipped = cases
        .iter()
        .filter(|node| {
            node.children().any(|child| child.has_tag_name("skipped")) || node.attribute("status") == Some("notrun")
        })
        .count();
    if cases.is_empty() || cases.len() == skipped {
        return Err(gate("CTest executed no tests; all selected tests were skipped"));
    }
    if cases.iter().any(|node| {
        node.children()
            .any(|child| child.has_tag_name("failure") || child.has_tag_name("error"))
    }) {
        return Err(gate("CTest JUnit contains failures"));
    }
    Ok(json!({"selected":tests.len(),"executed":cases.len()-skipped,"skipped":skipped,"junit":junit,"preset":preset}))
}
