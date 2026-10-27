// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Command-line contract.
use std::path::PathBuf;

use clap::Parser;
use clap::Subcommand;
/// CLI options shared with the cpp-infra tool bundle.
#[derive(Debug, Parser)]
#[command(name = "cpp-infra-verify", disable_version_flag = true)]
pub struct Cli {
    #[arg(long, global = true, default_value = ".")]
    pub(crate) project: PathBuf,
    #[arg(long, global = true)]
    pub(crate) report: Option<PathBuf>,
    #[arg(long, global = true)]
    pub(crate) offline: bool,
    #[arg(long, global = true)]
    pub(crate) run_context: Option<PathBuf>,
    #[arg(long, global = true)]
    pub(crate) version: bool,
    #[arg(long, global = true)]
    pub(crate) json: bool,
    #[command(subcommand)]
    pub(crate) command: Option<Action>,
}
#[derive(Debug, Subcommand)]
pub(crate) enum Action {
    Plan {
        #[arg(long)]
        profile: String,
    },
    Prepare {
        #[arg(long)]
        profile: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Run {
        #[arg(long)]
        profile: String,
        #[arg(long, value_delimiter=',', action=clap::ArgAction::Append)]
        suite: Vec<String>,
    },
}
impl Action {
    pub(crate) fn profile(&self) -> &str {
        match self {
            Self::Plan { profile } | Self::Prepare { profile, .. } | Self::Run { profile, .. } => profile,
        }
    }
}
