// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Project configuration files must belong to the shared digest's tracked area.
use std::fs;
use std::path::Component;
use std::path::Path;

use crate::verify_error::Result;
use crate::verify_error::config;
fn excluded(relative: &Path) -> bool {
    let mut components = relative.components();
    let first = components.next().map(|part| part.as_os_str());
    if first.is_some_and(|value| [".git", "build", "target"].iter().any(|name| value == *name)) {
        return true;
    }
    first.is_some_and(|value| value == ".infra")
        && components.next().is_some_and(|part| {
            [".tmp", "transactions", ".transactions"]
                .iter()
                .any(|name| part.as_os_str() == *name)
        })
}
pub(crate) fn validate(root: &Path, path: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| config("configuration input must belong to the project"))?;
    let mut normalized = std::path::PathBuf::new();
    for part in relative.components() {
        match part {
            Component::Normal(name) => normalized.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(config("configuration input escapes project"));
                }
            }
            _ => return Err(config("unsafe configuration input")),
        }
    }
    if excluded(&normalized) {
        return Err(config("configuration input cannot be in a digest exclusion area"));
    }
    reject_symlinks(root, path)?;
    let actual = path.canonicalize()?;
    let actual_relative = actual
        .strip_prefix(root)
        .map_err(|_| config("configuration input target escapes project"))?;
    if excluded(actual_relative) {
        return Err(config(
            "configuration input target cannot be in a digest exclusion area",
        ));
    }
    if !fs::metadata(&actual)?.is_file() {
        return Err(config("configuration input must be an ordinary file"));
    }
    Ok(())
}

pub(crate) fn reject_symlinks(root: &Path, path: &Path) -> Result<()> {
    // Config files deliberately use the stricter, non-symlink input policy.
    for ancestor in path.ancestors() {
        if ancestor == root {
            break;
        }
        if fs::symlink_metadata(ancestor)?.file_type().is_symlink() {
            return Err(config("project configuration inputs cannot use symlinks"));
        }
    }
    Ok(())
}
