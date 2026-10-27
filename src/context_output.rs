// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Build-context outputs have a separate role from source inputs and reports.
use std::fs;
use std::path::Path;

use crate::build_context::BuildContext;
use crate::config::canonical_future;
use crate::verify_error::Result;
use crate::verify_error::config;
pub(crate) fn validate(root: &Path, path: &Path) -> Result<()> {
    if path.to_string_lossy().ends_with(['/', '\\']) {
        return Err(config("build-context output must name an ordinary JSON file"));
    }
    let full = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    if full.extension().and_then(|extension| extension.to_str()) != Some("json") {
        return Err(config("build-context output must be a .json file under project build"));
    }
    let actual = canonical_future(&full)?;
    if !actual.starts_with(root.join("build")) || actual.starts_with(root.join("build/infra")) {
        return Err(config(
            "build-context output must be under project build, outside CMake binary directories",
        ));
    }
    if full.starts_with(root) && actual != full {
        return Err(config("build-context output cannot use project symlink aliases"));
    }
    if fs::symlink_metadata(&full).is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file()) {
        return Err(config("build-context output must be an ordinary file"));
    }
    if actual.exists() {
        let previous: BuildContext = serde_json::from_slice(&fs::read(&actual)?)
            .map_err(|_| config("refusing to overwrite a file that is not a build-context"))?;
        if previous.schema != 1
            || previous.project != root
            || !previous.binary_dir.starts_with(root.join("build/infra"))
            || previous.profile.is_empty()
            || previous.fingerprint.len() != 64
        {
            return Err(config(
                "existing build-context output belongs to a different project or role",
            ));
        }
    }
    Ok(())
}
