// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Requested suites and their deduplicated build prerequisites.
use std::collections::BTreeSet;

use serde_json::Value;
use serde_json::json;

use crate::config::Config;
use crate::verify_error::Result;
use crate::verify_error::config;
pub(crate) fn select(cfg: &Config, requested: &[String]) -> Result<Vec<String>> {
    let selected = if requested.is_empty() {
        cfg.required_suites.clone()
    } else {
        requested.to_vec()
    };
    let mut seen = BTreeSet::new();
    for name in &selected {
        if !seen.insert(name) {
            return Err(config(format!("duplicate requested suite: {name}")));
        }
        let enabled = match name.as_str() {
            "build" | "test" => true,
            "headers" => cfg.headers.is_some(),
            "install" => cfg.install.is_some(),
            "package" => cfg.package.is_some(),
            "sanitize" => cfg.sanitize.is_some(),
            "fuzz" => cfg.fuzz.is_some(),
            _ => false,
        };
        if !enabled {
            return Err(config(format!("unsupported or unconfigured suite: {name}")));
        }
    }
    if selected.is_empty() {
        return Err(config("no suites selected"));
    }
    Ok(selected)
}
pub(crate) fn plan(profile: &str, configure: &str, build: &str, selected: &[String], timeout: u64) -> Value {
    let mut nodes = vec![
        json!({"id":"configure","tool":"cmake","args":["--preset",configure],"profile":profile,"needs":[],"resource_locks":["binary_dir/.cpp-infra.lock"],"timeout_seconds":timeout,"required":true,"output":"runtime discovery"}),
        json!({"id":"build","tool":"cmake","args":["--build","--preset",build],"profile":profile,"needs":["configure"],"resource_locks":["binary_dir/.cpp-infra.lock"],"timeout_seconds":timeout,"required":true,"output":"build-context.json"}),
    ];
    for suite in selected {
        if suite != "build" {
            nodes.push(json!({"id":suite,"tool":"cpp-infra-verify","args":["run","--profile",profile,"--suite",suite],"profile":profile,"needs":["build"],"resource_locks":["binary_dir/.cpp-infra.lock"],"timeout_seconds":timeout,"required":true,"output":"runtime discovery"}));
        }
    }
    json!({"schema":1,"nodes":nodes})
}
