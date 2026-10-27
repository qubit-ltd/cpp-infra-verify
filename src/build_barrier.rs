// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Keep successful build ownership and state stable throughout consumption.
use std::path::Path;

use serde_json::Value;
use serde_json::json;

use crate::build_context::BuildContext;
use crate::file_api;
use crate::verify_error::Result;
use crate::verify_error::config;

pub(crate) struct BuildBarrier {
    state: Value,
    marker: Value,
}
fn read(root: &Path, path: &Path) -> Result<Value> {
    if let Err(error) = crate::project_input::reject_symlinks(root, path) {
        return Err(config(format!("build ownership proof unavailable: {error}")));
    }
    match file_api::json_file(path) {
        Ok(value) => Ok(value),
        Err(error) => Err(config(format!("build ownership proof unavailable: {error}"))),
    }
}
impl BuildBarrier {
    pub(crate) fn capture(context: &BuildContext) -> Result<Self> {
        let root = &context.project;
        let marker = read(root, &context.binary_dir.join(".cpp-infra-project.json"))?;
        if marker["schema"] != 1 || marker["project"] != json!(root) {
            return Err(config("build directory management marker mismatch"));
        }
        let state = read(root, &context.binary_dir.join(".cpp-infra-build-state.json"))?;
        if state["schema"] != 1
            || state["status"] != "built"
            || state["project"] != json!(root)
            || state["profile"] != context.profile
            || state["source_digest"] != context.source_digest
            || state["config_digest"] != context.config_digest
            || state["source_excluded_paths"] != json!(context.source_excluded_paths)
            || state["fingerprint"] != context.fingerprint
            || state["configuration_inputs"] != json!(context.configuration_inputs)
            || state["cmake_cache_digest"] != context.cmake_cache_digest
        {
            return Err(config(
                "prepared build state fingerprint changed or identity/proof/status mismatch",
            ));
        }
        crate::cmake_engine::previous(root, &context.binary_dir)?;
        Ok(Self { state, marker })
    }
    pub(crate) fn validate(&self, context: &BuildContext) -> Result<()> {
        let root = &context.project;
        let state = read(root, &context.binary_dir.join(".cpp-infra-build-state.json"))?;
        let marker = read(root, &context.binary_dir.join(".cpp-infra-project.json"))?;
        if state != self.state || marker != self.marker {
            return Err(config("prepared build state or ownership changed during verification"));
        }
        crate::cmake_engine::previous(root, &context.binary_dir)?;
        Ok(())
    }
}
