// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Verify a main-tracking invocation's immutable runtime tool snapshot.
use std::path::Path;

use serde_json::Value;

use crate::digest;
use crate::file_api::json_file;
use crate::report;
use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::missing_config;
pub(crate) fn verify(context: &Value, report_root: &Path) -> Result<()> {
    let snapshot = context.get("runtime_snapshot");
    let expected_digest = context.get("runtime_snapshot_digest");
    if snapshot.is_none() && expected_digest.is_none() {
        return Ok(());
    }
    let snapshot = Path::new(
        snapshot
            .and_then(Value::as_str)
            .ok_or_else(missing_config("runtime_snapshot must be an absolute path"))?,
    );
    if !snapshot.is_absolute() || !snapshot.canonicalize()?.starts_with(report_root.canonicalize()?) {
        return Err(config("runtime snapshot must be inside the controlled report root"));
    }
    if digest::file(snapshot)?
        != expected_digest
            .and_then(Value::as_str)
            .ok_or_else(missing_config("runtime_snapshot_digest missing"))?
    {
        return Err(config("runtime snapshot digest mismatch"));
    }
    let value = json_file(snapshot)?;
    if value["schema"] != 1 || value["bundle_id"] != context["bundle_id"] {
        return Err(config("runtime snapshot schema/bundle mismatch"));
    }
    let tools = value["tools"]
        .as_array()
        .ok_or_else(missing_config("runtime snapshot tools missing"))?;
    let matches: Vec<_> = tools.iter().filter(|tool| tool["name"] == "cpp-infra-verify").collect();
    if matches.len() != 1 {
        return Err(config("runtime snapshot must identify exactly one cpp-infra-verify"));
    }
    let own = matches[0];
    let identity = report::version();
    if own["protocol"] != 1 || own["version"] != identity["version"] || own["revision"] != identity["revision"] {
        return Err(config("runtime snapshot tool version/revision/protocol mismatch"));
    }
    let binary = Path::new(
        own["binary"]
            .as_str()
            .ok_or_else(missing_config("runtime snapshot executable path missing"))?,
    );
    let running = std::env::current_exe()?.canonicalize()?;
    if !binary.is_absolute()
        || own["executable_sha256"] != digest::file(binary)?
        || own["executable_sha256"] != digest::file(&running)?
    {
        return Err(config("runtime snapshot executable identity mismatch"));
    }
    Ok(())
}
