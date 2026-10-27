// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Build context consumed by analysis and coverage tools.
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
/// The verified result of configuring and building one committed preset.
#[derive(Debug, Serialize, Deserialize)]
pub struct BuildContext {
    /// Protocol schema, currently one.
    pub schema: u32,
    /// Canonical source directory.
    pub project: PathBuf,
    /// Project profile name.
    pub profile: String,
    /// Managed CMake binary directory.
    pub binary_dir: PathBuf,
    /// Committed configure preset.
    pub configure_preset: String,
    /// Committed build preset.
    pub build_preset: String,
    /// Optional committed CTest preset.
    pub test_preset: Option<String>,
    /// Explicit multi-configuration selection, if applicable.
    pub configuration: Option<String>,
    /// Canonical project source digest.
    pub source_digest: String,
    /// Explicit non-input report files omitted from source identity.
    #[serde(default)]
    pub source_excluded_paths: Vec<String>,
    /// Canonical project configuration digest.
    pub config_digest: String,
    /// Input and toolchain fingerprint.
    pub fingerprint: String,
    /// Actual CMake compiler information.
    pub compiler: serde_json::Value,
    /// Actual CMake generator.
    pub generator: String,
    /// Compilation database, if exported by the preset.
    pub compile_commands: Option<PathBuf>,
    /// File API target descriptions and absolute artifact/source paths.
    pub targets: Vec<serde_json::Value>,
    /// Prepared dependency metadata, revalidated before reusing built
    /// artifacts.
    #[serde(default)]
    pub dependency_context: Option<serde_json::Value>,
    /// Actual configuration inputs with ownership and content hashes.
    #[serde(default)]
    pub configuration_inputs: Vec<serde_json::Value>,
    /// Content identity of the configured CMake cache.
    #[serde(default)]
    pub cmake_cache_digest: String,
    /// Exact File API reply index used to produce this context.
    pub file_api_index: PathBuf,
}
