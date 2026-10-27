// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Stable error categories for CLI and library callers.
use std::fmt::Display;
use std::io;

use thiserror::Error;
/// A configuration, quality gate, or infrastructure failure.
#[derive(Debug, Error)]
pub enum VerifyError {
    /// Invalid configuration or protocol input.
    #[error("{0}")]
    Config(String),
    /// A real build, test, or verification failed.
    #[error("{0}")]
    Gate(String),
    /// An operation on the filesystem or a process failed.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// Execution was cancelled.
    #[error("verification cancelled")]
    Cancelled,
}
impl VerifyError {
    /// Returns the common cpp-infra exit code.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Config(_) => 2,
            Self::Gate(_) => 1,
            Self::Io(_) => 3,
            Self::Cancelled => 130,
        }
    }
}
pub(crate) type Result<T> = std::result::Result<T, VerifyError>;
pub(crate) fn config(message: impl Into<String>) -> VerifyError {
    VerifyError::Config(message.into())
}
pub(crate) fn gate(message: impl Into<String>) -> VerifyError {
    VerifyError::Gate(message.into())
}

/// Defers repeated missing-field diagnostics without eager error allocation.
pub(crate) fn missing_config(message: &'static str) -> impl FnOnce() -> VerifyError {
    move || config(message)
}
/// Converts a context-free protocol error while preserving its display text.
pub(crate) fn protocol_error(error: impl Display) -> VerifyError {
    config(error.to_string())
}
