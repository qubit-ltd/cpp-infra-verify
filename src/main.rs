// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Thin CLI adapter for the verification library.
use clap::Parser;
use qubit_cpp_infra_verify::Cli;
use qubit_cpp_infra_verify::execute;
fn main() {
    let cli = Cli::parse();
    if let Err(error) = execute(cli) {
        eprintln!("{error}");
        std::process::exit(error.exit_code());
    }
}
