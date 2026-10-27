// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Cross-tool path/content fingerprints.
use std::fs;
use std::path::Path;

use sha2::Digest;
use sha2::Sha256;
use walkdir::WalkDir;

use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::missing_config;
use crate::verify_error::protocol_error;
pub(crate) fn bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub(crate) fn file(path: &Path) -> Result<String> {
    Ok(bytes(&fs::read(path)?))
}
pub(crate) fn project(root: &Path, configs: bool, excluded: &[String]) -> Result<String> {
    let mut files = Vec::new();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            let path = entry.path();
            if path == root {
                return true;
            }

            let relative = path.strip_prefix(root).unwrap_or(path);
            if configs
                && [".infra/.tmp", ".infra/transactions", ".infra/.transactions"]
                    .iter()
                    .any(|directory| relative == Path::new(directory))
            {
                return false;
            }
            let first = relative.components().next().map(|p| p.as_os_str().to_string_lossy());
            !matches!(first.as_deref(), Some(".git" | "build" | "target"))
                && (configs || first.as_deref() != Some(".infra"))
        });
    for entry in walker {
        let entry = entry.map_err(|e| config(format!("digest traversal: {e}")))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .map_err(protocol_error)?
            .to_str()
            .ok_or_else(missing_config("non UTF-8 project path"))?
            .replace('\\', "/");
        if !configs && excluded.binary_search(&rel).is_ok() {
            continue;
        }
        if configs
            && !(rel.starts_with(".infra/")
                || ["CMakeLists.txt", "CMakePresets.json", "conanfile.py", "conanfile.txt"].contains(&rel.as_str())
                || rel.starts_with("conan/"))
        {
            continue;
        }
        if configs
            && (rel.starts_with(".infra/.tmp/")
                || rel.starts_with(".infra/transactions/")
                || rel.starts_with(".infra/.transactions/"))
        {
            continue;
        }
        files.push((rel, entry.into_path()));
    }
    files.sort_by(|(left, _), (right, _)| left.as_bytes().cmp(right.as_bytes()));
    let mut hash = Sha256::new();
    for (relative, path) in files {
        let content = fs::read(path)?;
        hash.update((relative.len() as u64).to_le_bytes());
        hash.update(relative.as_bytes());
        hash.update((content.len() as u64).to_le_bytes());
        hash.update(content);
    }
    Ok(hex::encode(hash.finalize()))
}
