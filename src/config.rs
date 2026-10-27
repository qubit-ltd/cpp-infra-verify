// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Strict project and verification configuration.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;

use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::missing_config;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Project {
    pub schema: u32,
    pub kind: String,
    #[serde(default)]
    pub source_roots: Vec<String>,
    #[serde(default)]
    pub test_roots: Vec<String>,
    #[serde(default)]
    pub generated_roots: Vec<String>,
    #[serde(default)]
    pub third_party_roots: Vec<String>,
    pub profiles: BTreeMap<String, Profile>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    pub configure: String,
    pub build: String,
    pub test: Option<String>,
    pub conan_context: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    pub schema: u32,
    pub required_suites: Vec<String>,
    #[serde(default = "configure_timeout")]
    pub configure_timeout_seconds: u64,
    #[serde(default = "build_timeout")]
    pub build_timeout_seconds: u64,
    #[serde(default = "test_timeout")]
    pub test_timeout_seconds: u64,
    #[serde(default = "jobs")]
    pub jobs: u32,
    pub headers: Option<Headers>,
    pub install: Option<Install>,
    pub package: Option<Package>,
    pub sanitize: Option<Sanitize>,
    pub fuzz: Option<Fuzz>,
}
fn configure_timeout() -> u64 {
    300
}
fn build_timeout() -> u64 {
    1800
}
fn test_timeout() -> u64 {
    600
}
fn jobs() -> u32 {
    4
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Headers {
    pub roots: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub exceptions: BTreeMap<String, String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Install {
    pub package: Option<String>,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub headers: Vec<String>,
    pub consumer_source: Option<String>,
    #[serde(default)]
    pub run_consumer: bool,
    #[serde(default)]
    pub executables: Vec<String>,
    #[serde(default)]
    pub smoke_args: Vec<String>,
    #[serde(default)]
    pub emulator: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Package {
    pub preset: String,
    pub required_files: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Sanitize {
    pub sanitizers: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fuzz {
    pub mode: String,
    pub targets: Vec<FuzzTarget>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FuzzTarget {
    pub target: String,
    pub seconds: u64,
    pub max_len: u64,
    pub corpus: String,
    pub timeout_seconds: u64,
}
pub(crate) fn load<T: serde::de::DeserializeOwned>(root: &Path, path: &Path) -> Result<T> {
    crate::project_input::validate(root, path)?;
    let text = fs::read_to_string(path)?;
    toml::from_str(&text).map_err(|error| config(format!("{}: {error}", path.display())))
}
pub(crate) fn relative_path(root: &Path, path: &str, must_exist: bool) -> Result<PathBuf> {
    let input = Path::new(path);
    if input.is_absolute()
        || input
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        return Err(config(format!("unsafe project path: {path}")));
    }
    let full = root.join(input);
    let mut ancestor = full.as_path();
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .ok_or_else(missing_config("path has no existing ancestor"))?;
    }
    if !ancestor.canonicalize()?.starts_with(root) {
        return Err(config(format!("path escapes project: {path}")));
    }
    if must_exist && !full.exists() {
        return Err(config(format!("configured path does not exist: {path}")));
    }
    Ok(full)
}
pub(crate) fn validate(root: &Path, project: &Project, cfg: &Config) -> Result<()> {
    if project.schema != 1 || cfg.schema != 1 {
        return Err(config("unsupported configuration schema"));
    }
    if !["library", "application", "header-only"].contains(&project.kind.as_str()) {
        return Err(config("unknown project kind"));
    }
    if cfg.jobs == 0
        || [
            cfg.configure_timeout_seconds,
            cfg.build_timeout_seconds,
            cfg.test_timeout_seconds,
        ]
        .contains(&0)
    {
        return Err(config("jobs and timeouts must be positive"));
    }
    let mut seen = BTreeSet::new();
    for suite in &cfg.required_suites {
        if !["build", "test", "headers", "install", "package", "sanitize", "fuzz"].contains(&suite.as_str())
            || !seen.insert(suite)
        {
            return Err(config(format!("invalid or duplicate suite: {suite}")));
        }
    }
    if cfg.required_suites.is_empty() {
        return Err(config("required_suites must not be empty"));
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for path in project
        .source_roots
        .iter()
        .chain(&project.test_roots)
        .chain(&project.generated_roots)
        .chain(&project.third_party_roots)
    {
        let full = relative_path(root, path, true)?.canonicalize()?;
        if roots
            .iter()
            .any(|other| other.starts_with(&full) || full.starts_with(other))
        {
            return Err(config(format!("overlapping project root: {path}")));
        }
        roots.push(full);
    }
    if project.kind != "application" && cfg.install.is_none() {
        return Err(config("library and header-only projects require install configuration"));
    }
    if let Some(headers) = &cfg.headers {
        for path in &headers.roots {
            let full = relative_path(root, path, true)?.canonicalize()?;
            if !project
                .source_roots
                .iter()
                .any(|source| full.starts_with(root.join(source)))
            {
                return Err(config("header roots must be within source_roots"));
            }
        }
        for (path, host) in &headers.exceptions {
            relative_path(root, path, true)?;
            relative_path(root, host, true)?;
        }
        for path in &headers.exclude {
            relative_path(root, path, true)?;
            if !headers.exceptions.contains_key(path) {
                return Err(config("excluded header needs an exact exceptions host header"));
            }
        }
    }
    if let Some(install) = &cfg.install {
        if let Some(source) = &install.consumer_source {
            relative_path(root, source, true)?;
        }
        if project.kind == "application" && install.executables.is_empty() {
            return Err(config("application install requires executables"));
        }
        if project.kind != "application"
            && (install.package.is_none() || install.targets.is_empty() || install.consumer_source.is_none())
        {
            return Err(config("library install requires package, targets and consumer_source"));
        }
        for path in install.headers.iter().chain(&install.executables) {
            relative_path(root, path, false)?;
        }
    }
    if let Some(package) = &cfg.package {
        if package.preset.is_empty() || package.required_files.is_empty() {
            return Err(config("package requires preset and nonempty required_files"));
        }
        for path in &package.required_files {
            relative_path(root, path, false)?;
        }
    }
    if let Some(sanitize) = &cfg.sanitize {
        let selected: BTreeSet<&str> = sanitize.sanitizers.iter().map(String::as_str).collect();
        if selected.len() != sanitize.sanitizers.len()
            || !(selected == BTreeSet::from(["address", "undefined"]) || selected == BTreeSet::from(["thread"]))
        {
            return Err(config(
                "supported sanitizer sets: address+undefined, or thread separately",
            ));
        }
    }
    if let Some(fuzz) = &cfg.fuzz {
        if !["build-only", "smoke"].contains(&fuzz.mode.as_str()) || fuzz.targets.is_empty() {
            return Err(config("invalid fuzz mode or empty targets"));
        }
        let mut targets = BTreeSet::new();
        for target in &fuzz.targets {
            if target.seconds == 0
                || target.max_len == 0
                || target.timeout_seconds == 0
                || !targets.insert(&target.target)
            {
                return Err(config("invalid fuzz budget or duplicate target"));
            }
            relative_path(root, &target.corpus, true)?;
        }
    }
    Ok(())
}

pub(crate) fn canonical_future(path: &Path) -> Result<PathBuf> {
    let mut existing = path;
    let mut suffix = Vec::new();
    while !existing.exists() {
        suffix.push(
            existing
                .file_name()
                .ok_or_else(missing_config("path has no ancestor"))?
                .to_owned(),
        );
        existing = existing.parent().ok_or_else(missing_config("path has no parent"))?;
    }
    let mut canonical = existing.canonicalize()?;
    for component in suffix.iter().rev() {
        canonical.push(component);
    }
    Ok(canonical)
}
