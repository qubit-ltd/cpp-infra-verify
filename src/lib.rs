// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! CMake verification, isolated consumer suites, and reusable build contexts.
mod build_barrier;
mod build_context;
mod cancellation;
mod cli;
mod cmake_engine;
mod cmake_inputs;
mod config;
mod context_output;
mod ctest;
mod digest;
mod file_api;
mod fuzz;
mod headers;
mod install;
mod package;
mod presets;
mod project_input;
mod report;
mod report_paths;
mod runner;
mod runtime_snapshot;
mod sanitize;
mod suite_graph;
mod verification;
mod verify_error;
pub use build_context::BuildContext;
pub use cli::Cli;
pub use verification::execute;
pub use verify_error::VerifyError;
