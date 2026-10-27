// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Validate explicit non-input JSON reports before excluding them from source
//! identity.
use std::collections::BTreeSet;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;

use crate::config::Project;
use crate::config::canonical_future;
use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::missing_config;
fn public_report(path: &Path) -> Result<bool> {
    let value: Value = match serde_json::from_slice(&fs::read(path)?) {
        Ok(value) => value,
        Err(_) => return Ok(false),
    };
    let keys = [
        "schema",
        "tool",
        "bundle_id",
        "run_id",
        "task_id",
        "project",
        "report_root",
        "source_revision",
        "source_digest",
        "config_digest",
        "status",
        "started_at",
        "duration_ms",
        "external_versions",
        "commands",
        "diagnostics",
        "artifacts",
    ];
    Ok(value["schema"] == 1
        && keys.iter().all(|key| value.get(key).is_some())
        && value["tool"]["name"].as_str().is_some_and(|name| {
            [
                "cpp-infra-tools",
                "cpp-infra-ci",
                "cpp-infra-verify",
                "cpp-infra-style",
                "cpp-infra-analyze",
                "cpp-infra-dependency",
                "cpp-infra-coverage",
                "cpp-infra-pages",
            ]
            .contains(&name)
        })
        && value["commands"].is_array()
        && value["diagnostics"].is_array()
        && value["artifacts"].is_array())
}
fn normalized(name: &str) -> Result<PathBuf> {
    let input = Path::new(name);
    if name.as_bytes().get(1) == Some(&b':') && name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
        return Err(config("source_excluded_paths cannot contain Windows drive paths"));
    }
    if name.is_empty()
        || name.contains('\\')
        || input.is_absolute()
        || input.components().any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(config("source_excluded_paths requires normalized relative file paths"));
    }
    let normalized = input
        .components()
        .map(|part| {
            part.as_os_str()
                .to_str()
                .ok_or_else(missing_config("non UTF-8 report path"))
        })
        .collect::<Result<Vec<_>>>()?
        .join("/");
    if normalized != name || input.extension().and_then(|value| value.to_str()) != Some("json") {
        return Err(config("excluded report must be a normalized .json file"));
    }
    Ok(input.to_owned())
}
fn validate(root: &Path, project: &Project, name: &str) -> Result<String> {
    let relative = normalized(name)?;
    let full = root.join(&relative);
    let actual = canonical_future(&full)?;
    if actual != full || !actual.starts_with(root) {
        return Err(config("excluded report cannot use symlink aliases"));
    }
    if actual.is_dir() {
        return Err(config("excluded report must be a file"));
    }
    let first = relative
        .components()
        .next()
        .ok_or_else(missing_config("empty report path"))?;
    if [".git", ".infra", "conan"]
        .iter()
        .any(|name| first.as_os_str() == *name)
    {
        return Err(config("report path is a protected configuration input"));
    }
    let basename = relative
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(missing_config("non UTF-8 report path"))?
        .to_ascii_lowercase();
    if ["cmake", "conan", "readme"]
        .iter()
        .any(|prefix| basename.starts_with(prefix))
    {
        return Err(config("report path is a reserved project input"));
    }
    for directory in project
        .source_roots
        .iter()
        .chain(&project.test_roots)
        .chain(&project.generated_roots)
        .chain(&project.third_party_roots)
    {
        let protected = canonical_future(&root.join(directory))?;
        if actual.starts_with(&protected) {
            return Err(config("report path is inside a configured input root"));
        }
    }
    if full.exists() && !public_report(&full)? {
        return Err(config("refusing to overwrite or exclude an unknown business JSON file"));
    }
    Ok(name.to_owned())
}
fn output(root: &Path, project: &Project, path: &Path) -> Result<Option<String>> {
    let full = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let actual = canonical_future(&full)?;
    if full.starts_with(root) && full != actual {
        return Err(config("report output cannot use a project symlink alias"));
    }
    let Ok(relative) = actual.strip_prefix(root) else {
        return Ok(None);
    };
    let first = relative
        .components()
        .next()
        .ok_or_else(missing_config("report output must be a file"))?;
    // Build/target are already omitted from the source digest, including default
    // outputs.
    if first.as_os_str() == "build" || first.as_os_str() == "target" {
        if actual.is_dir() {
            return Err(config("report output must be a file"));
        }
        return Ok(None);
    }
    let name = relative.to_str().ok_or_else(missing_config("non UTF-8 report path"))?;
    Ok(Some(validate(root, project, name)?))
}
pub(crate) fn resolve(
    root: &Path,
    project: &Project,
    incoming: Option<&Value>,
    report: Option<&Path>,
) -> Result<Vec<String>> {
    let mut excluded = BTreeSet::new();
    if let Some(incoming) = incoming {
        for value in incoming
            .as_array()
            .ok_or_else(missing_config("source_excluded_paths must be an array"))?
        {
            let name = value
                .as_str()
                .ok_or_else(missing_config("source_excluded_paths items must be strings"))?;
            excluded.insert(validate(root, project, name)?);
        }
    }
    if let Some(path) = report
        && let Some(name) = output(root, project, path)?
    {
        excluded.insert(name);
    }
    Ok(excluded.into_iter().collect())
}

pub(crate) fn basic_output_safe(root: &Path, path: &Path) -> Result<bool> {
    let full = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let actual = canonical_future(&full)?;
    if full.starts_with(root) && full != actual {
        return Ok(false);
    }
    let Ok(relative) = actual.strip_prefix(root) else {
        return Ok(true);
    };
    Ok(relative
        .components()
        .next()
        .is_some_and(|part| part.as_os_str() == "build" || part.as_os_str() == "target"))
}
