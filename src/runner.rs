// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Command execution with bounded runtime, logs, and process-tree termination.
use std::fs;
use std::fs::File;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

use serde_json::Value;
use serde_json::json;
use wait_timeout::ChildExt;

use crate::verify_error::Result;
use crate::verify_error::gate;
#[derive(Debug)]
pub(crate) struct Runner {
    pub commands: Vec<Value>,
    pub report_root: PathBuf,
    pub work_root: PathBuf,
}
impl Runner {
    pub(crate) fn run(
        &mut self,
        program: &str,
        args: &[String],
        cwd: &Path,
        timeout: u64,
        env: &[(String, String)],
    ) -> Result<String> {
        let sequence = self.commands.len();
        let log = self
            .work_root
            .strip_prefix(&self.report_root)
            .map_err(|_| crate::verify_error::config("workspace outside report root"))?
            .join(format!("logs/{sequence:04}.log"));
        let full = self.report_root.join(&log);
        fs::create_dir_all(full.parent().unwrap_or(&self.report_root))?;
        let stdout = File::create(&full)?;
        let stderr = stdout.try_clone()?;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .envs(env.iter().map(|(key, value)| (key, value)));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let started = Instant::now();
        eprintln!("running {program} {}", args.join(" "));
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.commands.push(json!({"program":program,"args":args,"cwd":cwd,"exit_code":null,"duration_ms":started.elapsed().as_millis(),"log":log,"env":redact(env)}));
                return Err(error.into());
            }
        };
        let mut timed_out = false;
        let mut cancelled = false;
        let status = loop {
            if crate::cancellation::cancelled() {
                cancelled = true;
                crate::cancellation::terminate_tree(child.id());
                break child.wait()?;
            }
            if started.elapsed() >= Duration::from_secs(timeout) {
                timed_out = true;
                crate::cancellation::terminate_tree(child.id());
                break child.wait()?;
            }
            if let Some(status) = child.wait_timeout(Duration::from_millis(100))? {
                break status;
            }
        };
        self.commands.push(json!({"program":program,"args":args,"cwd":cwd,"exit_code":status.code(),"duration_ms":started.elapsed().as_millis(),"log":log,"env":redact(env)}));
        let output = fs::read_to_string(&full)?;
        if cancelled {
            return Err(crate::verify_error::VerifyError::Cancelled);
        }
        if timed_out {
            return Err(gate(format!(
                "command timed out after {timeout}s: {program}; log {}",
                full.display()
            )));
        }
        if !status.success() {
            eprintln!("{output}");
            return Err(gate(format!(
                "{program} exited {:?}; log {}",
                status.code(),
                full.display()
            )));
        }
        Ok(output)
    }
}
fn redact(env: &[(String, String)]) -> Value {
    let mut values = serde_json::Map::new();
    for (key, value) in env {
        let name = key.to_ascii_uppercase();
        let sensitive = ["TOKEN", "KEY", "PASS", "SECRET", "CREDENTIAL"]
            .iter()
            .any(|needle| name.contains(needle));
        values.insert(key.clone(), json!(if sensitive { "<redacted>" } else { value }));
    }
    Value::Object(values)
}
pub(crate) fn tool(name: &str) -> Result<String> {
    if let Some(directory) = std::env::var_os("CPP_INFRA_BIN_DIR") {
        let path = Path::new(&directory).join(name);
        if !path.is_file() {
            return Err(crate::verify_error::config(format!(
                "bundle missing required tool: {}",
                path.display()
            )));
        }
        Ok(path.to_string_lossy().into_owned())
    } else {
        Ok(name.into())
    }
}
