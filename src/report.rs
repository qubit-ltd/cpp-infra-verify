// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Common atomic JSON report and artifact operations.
use std::fs;
use std::path::Path;

use serde_json::Value;
use serde_json::json;

use crate::digest;
use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::protocol_error;
pub(crate) fn write(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temporary, serde_json::to_vec_pretty(value).map_err(protocol_error)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}
pub(crate) fn artifact(root: &Path, path: &Path, kind: &str) -> Result<Value> {
    let full = path.canonicalize()?;
    let relative = full
        .strip_prefix(root.canonicalize()?)
        .map_err(|_| config("artifact outside report_root"))?
        .to_path_buf();
    Ok(json!({"kind":kind,"path":relative,"sha256":digest::file(&full)?,"required":true}))
}
pub(crate) fn version() -> Value {
    json!({"name":"cpp-infra-verify","version":env!("CARGO_PKG_VERSION"),"revision":option_env!("CPP_INFRA_BUILD_REVISION")})
}
