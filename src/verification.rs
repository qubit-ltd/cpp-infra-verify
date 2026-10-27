// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Coordinate verified configuration, dependencies, locks, reports, and suites.
use std::fs;
use std::fs::OpenOptions;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;

use fs2::FileExt;
use serde_json::Value;
use serde_json::json;

use crate::cli::Action;
use crate::cli::Cli;
use crate::config::Config;
use crate::config::Project;
use crate::config::load;
use crate::config::validate;
use crate::digest;
use crate::file_api;
use crate::presets;
use crate::report;
use crate::runner::Runner;
use crate::runner::tool;
use crate::suite_graph;
use crate::verify_error::Result;
use crate::verify_error::VerifyError;
use crate::verify_error::config;
use crate::verify_error::missing_config;
use crate::verify_error::protocol_error;
static COUNTER: AtomicU64 = AtomicU64::new(0);
/// Executes a CLI request and writes a common JSON report even on most
/// failures.
///
/// Configures/builds CMake projects and creates run artifacts under
/// `build/infra-runs`. Returns a typed configuration, gate, cancellation, or
/// I/O error. Commands have configured timeouts and hold an exclusive
/// build-directory lock while consuming mutable artifacts. A supplied run
/// context must match the current source and configuration digests.
pub fn execute(cli: Cli) -> std::result::Result<(), VerifyError> {
    crate::cancellation::install();
    if cli.version {
        let mut version = report::version();
        version["protocol"] = json!(1);
        if cli.json {
            println!("{}", version);
        } else {
            println!("cpp-infra-verify {}", env!("CARGO_PKG_VERSION"));
        }
        return Ok(());
    }
    let action = cli
        .command
        .as_ref()
        .ok_or_else(missing_config("a command is required; see --help"))?;
    let started = Instant::now();
    let started_at = chrono::Utc::now().to_rfc3339();
    let project = cli.project.canonicalize()?;
    if !project.is_dir() {
        return Err(config("--project must be a directory"));
    }
    let run_id = format!(
        "{}-{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ"),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let mut state = json!({"schema":1,"tool":report::version(),"bundle_id":"development","run_id":run_id,"task_id":"verify","project":project,"report_root":project.join("build/infra-runs").join(&run_id),"source_revision":source_revision(&project),"source_digest":null,"config_digest":null,"status":"error","started_at":started_at,"duration_ms":0,"external_versions":{},"commands":[],"diagnostics":[],"artifacts":[],"data":{}});
    let default_root = project.join("build/infra-runs").join(&run_id);
    let mut runner = Runner {
        commands: Vec::new(),
        report_root: default_root.clone(),
        work_root: default_root.join("verify").join(&run_id),
    };
    let mut output_validated = cli
        .report
        .as_deref()
        .map(|path| crate::report_paths::basic_output_safe(&project, path).unwrap_or(false))
        .unwrap_or_else(|| {
            crate::report_paths::basic_output_safe(&project, &runner.report_root.join("report.json")).unwrap_or(false)
        });
    let mut result = run(&cli, action, &project, &mut state, &mut runner, &mut output_validated);
    if let Err(error) = collect_artifacts(&mut state, &runner) {
        result = Err(error);
    }
    state["report_root"] = json!(runner.report_root);
    state["commands"] = json!(runner.commands);
    state["duration_ms"] = json!(started.elapsed().as_millis());
    state["status"] = json!(match &result {
        Ok(()) => "passed",
        Err(VerifyError::Gate(_)) => "failed",
        Err(VerifyError::Cancelled) => "cancelled",
        Err(_) => "error",
    });
    if let Err(error) = &result {
        state["diagnostics"].as_array_mut().ok_or_else(missing_config("report diagnostics invariant"))?.push(json!({"code":match error{VerifyError::Config(_)=>"VERIFY_CONFIG",VerifyError::Gate(_)=>"VERIFY_GATE",VerifyError::Io(_)=>"VERIFY_IO",VerifyError::Cancelled=>"VERIFY_CANCELLED"},"severity":"error","message":error.to_string(),"project_relative_path":null}));
    }
    if !output_validated {
        return result.and(Err(config("unsafe report output")));
    }
    let destination = cli
        .report
        .clone()
        .unwrap_or_else(|| runner.report_root.join("report.json"));
    report::write(&destination, &state)?;
    result
}
fn source_revision(root: &Path) -> Option<String> {
    let git = root
        .ancestors()
        .map(|directory| directory.join(".git"))
        .find(|path| path.exists())?;
    let directory = if git.is_dir() {
        git
    } else {
        let text = fs::read_to_string(&git).ok()?;
        git.parent()?.join(text.trim().strip_prefix("gitdir: ")?)
    };
    let head = fs::read_to_string(directory.join("HEAD")).ok()?;
    let revision = if let Some(reference) = head.trim().strip_prefix("ref: ") {
        if Path::new(reference).is_absolute()
            || Path::new(reference)
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return None;
        }
        fs::read_to_string(directory.join(reference)).ok().or_else(|| {
            fs::read_to_string(directory.join("packed-refs"))
                .ok()?
                .lines()
                .find_map(|line| {
                    let (hash, name) = line.split_once(' ')?;
                    (name == reference).then(|| hash.to_owned())
                })
        })?
    } else {
        head
    };
    let revision = revision.trim();
    (revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| revision.to_owned())
}
fn safe_component(value: &str) -> bool {
    Path::new(value).components().count() == 1
        && Path::new(value)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}
fn run(
    cli: &Cli,
    action: &Action,
    root: &Path,
    state: &mut Value,
    runner: &mut Runner,
    output_validated: &mut bool,
) -> Result<()> {
    let project: Project = load(root, &root.join(".infra/project.toml"))?;
    crate::project_input::validate(root, &root.join("CMakeLists.txt"))?;
    let default_report = runner.report_root.join("report.json");
    crate::report_paths::resolve(
        root,
        &project,
        None,
        Some(cli.report.as_deref().unwrap_or(&default_report)),
    )?;
    *output_validated = true;
    let inherited = cli
        .run_context
        .as_ref()
        .map(|path| file_api::json_file(path))
        .transpose()?;
    let excluded = crate::report_paths::resolve(
        root,
        &project,
        inherited.as_ref().and_then(|value| value.get("source_excluded_paths")),
        cli.report.as_deref(),
    )?;
    *output_validated = true;
    state["source_excluded_paths"] = json!(excluded);
    let source = digest::project(root, false, &excluded)?;
    let configuration = digest::project(root, true, &[])?;
    state["source_digest"] = json!(source);
    state["config_digest"] = json!(configuration);
    let mut reuse_path = None;
    if let Some(path) = &cli.run_context {
        let context = file_api::json_file(path)?;
        if context["schema"] != 1
            || context["project"]
                .as_str()
                .map(Path::new)
                .map(Path::canonicalize)
                .transpose()?
                .as_deref()
                != Some(root)
            || context["config_digest"] != configuration
            || context["source_digest"] != source
            || context["profile"] != action.profile()
        {
            return Err(config("run context project/profile/source/config fingerprint mismatch"));
        }
        for key in ["run_id", "task_id", "bundle_id"] {
            let value = context[key]
                .as_str()
                .ok_or_else(|| config(format!("run context missing {key}")))?;
            if !safe_component(value) {
                return Err(config(format!("unsafe run context {key}")));
            }
            state[key] = json!(value);
        }
        let report_root = PathBuf::from(
            context["report_root"]
                .as_str()
                .ok_or_else(missing_config("run context report_root missing"))?,
        );
        let relative = report_root
            .strip_prefix(root)
            .map_err(|_| config("run context report_root must be in project"))?;
        crate::config::relative_path(root, &relative.to_string_lossy(), false)?;
        if !report_root.starts_with(root.join("build/infra-runs")) {
            return Err(config("run context report_root outside build/infra-runs"));
        }
        if crate::config::canonical_future(&report_root)? != report_root {
            return Err(config("run context report root cannot use project symlink aliases"));
        }
        let invocation = runner
            .work_root
            .file_name()
            .ok_or_else(missing_config("workspace name"))?
            .to_owned();
        runner.work_root = report_root.join("verify").join(invocation);
        runner.report_root = report_root;
        crate::runtime_snapshot::verify(&context, &runner.report_root)?;
        if let Some(value) = context.get("build_context") {
            let path = PathBuf::from(
                value
                    .as_str()
                    .ok_or_else(missing_config("run-context build_context must be an absolute path"))?,
            );
            if !path.is_absolute() || !path.canonicalize()?.starts_with(runner.report_root.canonicalize()?) {
                return Err(config("build context outside controlled report root"));
            }
            reuse_path = Some(path);
        }
    }
    state["report_root"] = json!(runner.report_root);
    let cfg: Config = load(root, &root.join(".infra/verify/config.toml"))?;
    validate(root, &project, &cfg)?;
    let profile = project
        .profiles
        .get(action.profile())
        .ok_or_else(|| config(format!("unknown project profile: {}", action.profile())))?;
    let presets = presets::load(root, profile)?;
    if let Action::Prepare {
        output: Some(output), ..
    } = action
    {
        crate::context_output::validate(root, output)?;
    }
    let selected = match action {
        Action::Run { suite, .. } => suite_graph::select(&cfg, suite)?,
        Action::Prepare { .. } => vec!["build".into()],
        Action::Plan { .. } => suite_graph::select(&cfg, &[])?,
    };
    if selected.iter().any(|suite| suite == "package") {
        let package = cfg
            .package
            .as_ref()
            .ok_or_else(missing_config("package configuration missing"))?;
        presets::validate_package(root, &package.preset, &profile.configure)?;
    }
    if matches!(action, Action::Plan { .. }) {
        let plan = suite_graph::plan(
            action.profile(),
            &profile.configure,
            &profile.build,
            &selected,
            cfg.build_timeout_seconds,
        );
        println!("{plan}");
        state["data"]["plan"] = plan;
        return Ok(());
    }
    fs::create_dir_all(&runner.work_root)?;
    state["data"]["suites"] = json!(
        selected
            .iter()
            .map(|suite| json!({"suite":suite,"status":"blocked"}))
            .collect::<Vec<_>>()
    );
    let mut dependency = None;
    if reuse_path.is_none()
        && let Some(context) = &profile.conan_context
    {
        let output = runner.work_root.join("dependency-context.json");
        let report_path = runner.work_root.join("dependency-report.json");
        let mut args = vec![
            "--project".into(),
            root.to_string_lossy().into_owned(),
            "--report".into(),
            report_path.to_string_lossy().into_owned(),
        ];
        if cli.offline {
            args.push("--offline".into());
        }
        let mut child_context=inherited.clone().unwrap_or_else(||json!({"schema":1,"run_id":state["run_id"],"task_id":state["task_id"],"bundle_id":state["bundle_id"],"project":root,"profile":action.profile(),"source_digest":source,"config_digest":configuration,"report_root":runner.report_root}));
        child_context["source_excluded_paths"] = json!(excluded);
        let child_context_path = runner.work_root.join("dependency-run-context.json");
        report::write(&child_context_path, &child_context)?;
        args.extend([
            "--run-context".into(),
            child_context_path.to_string_lossy().into_owned(),
        ]);
        args.extend([
            "prepare".into(),
            "--context".into(),
            context.clone(),
            "--output".into(),
            output.to_string_lossy().into_owned(),
        ]);
        runner.run(
            &tool("cpp-infra-dependency")?,
            &args,
            root,
            cfg.configure_timeout_seconds,
            &[],
        )?;
        let value = file_api::json_file(&output)?;
        let child_report = file_api::json_file(&report_path)?;
        if child_report["status"] != "passed"
            || value["schema"] != 1
            || value["source_digest"] != source
            || value["config_digest"] != configuration
            || value["project"] != json!(root)
            || value["context"] != *context
            || value
                .get("source_excluded_paths")
                .is_some_and(|paths| *paths != json!(excluded))
        {
            return Err(config("dependency context/report fingerprint mismatch"));
        }
        dependency = Some(value);
    }
    fs::create_dir_all(&presets.binary_dir)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(presets.binary_dir.join(".cpp-infra.lock"))?;
    let waiting = Instant::now();
    loop {
        match FileExt::try_lock_exclusive(&lock) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if crate::cancellation::cancelled() {
                    return Err(VerifyError::Cancelled);
                }
                if waiting.elapsed().as_secs() >= cfg.configure_timeout_seconds {
                    return Err(config("timed out waiting for build directory lock"));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(error) => return Err(error.into()),
        }
    }

    let _lock = lock;
    verify_unchanged(root, &source, &configuration, &excluded, &presets.input_files)?;
    if let Some(dependency) = &dependency {
        verify_dependency(root, dependency)?;
    }
    let marker = presets.binary_dir.join(".cpp-infra-project.json");
    if marker.exists() {
        let value = file_api::json_file(&marker)?;
        if value["schema"] != 1 || value["project"] != json!(root) {
            return Err(config("build directory management marker mismatch"));
        }
    } else {
        let entries = fs::read_dir(&presets.binary_dir)?.collect::<std::io::Result<Vec<_>>>()?;
        if entries
            .iter()
            .any(|entry| entry.file_name() != ".cpp-infra.lock" && entry.file_name() != "generators")
        {
            return Err(config("refusing to take over unmarked nonempty build directory"));
        }
        report::write(&marker, &json!({"schema":1,"project":root}))?;
    }
    if presets.binary_dir.join("CMakeCache.txt").is_file() {
        let cache = fs::read_to_string(presets.binary_dir.join("CMakeCache.txt"))?;
        let expected = format!("CMAKE_HOME_DIRECTORY:INTERNAL={}", root.display());
        if !cache.lines().any(|line| line == expected) {
            return Err(config("existing CMake cache belongs to another source root"));
        }
    }
    let (context, cmake_inputs, barrier) = if let Some(path) = reuse_path {
        let mut stored: crate::BuildContext =
            serde_json::from_slice(&fs::read(&path)?).map_err(|error| config(format!("build context: {error}")))?;
        if stored.schema != 1
            || stored.project != root
            || stored.profile != action.profile()
            || stored.binary_dir != presets.binary_dir
            || stored.source_digest != source
            || stored.config_digest != configuration
            || stored.configure_preset != profile.configure
            || stored.build_preset != profile.build
            || stored.test_preset != profile.test
            || stored.configuration != presets.configuration
            || stored.source_excluded_paths != excluded
        {
            return Err(config(
                "prepared build context does not match this profile/project/presets",
            ));
        }
        let barrier = crate::build_barrier::BuildBarrier::capture(&stored)?;
        if profile.conan_context.is_some() != stored.dependency_context.is_some() {
            return Err(config("prepared dependency context does not match profile"));
        }
        if let Some(dependency) = &stored.dependency_context {
            verify_dependency(root, dependency)?;
            verify_toolchain(&stored, dependency)?;
        }
        let mut inputs = crate::cmake_inputs::CmakeInputs::new(root, &presets, stored.dependency_context.as_ref())?;
        let mut actual = file_api::read(root, action.profile(), profile, &presets, &source, &configuration)?;
        inputs.observe(&actual)?;
        inputs.validate()?;
        actual.configuration_inputs = inputs.records();
        if stored.configuration_inputs != actual.configuration_inputs
            || stored.cmake_cache_digest != actual.cmake_cache_digest
        {
            return Err(config("prepared configuration input proof changed"));
        }
        attach_dependency(&mut actual, stored.dependency_context.clone())?;
        attach_configuration(&mut actual)?;
        if actual.fingerprint != stored.fingerprint || actual.targets != stored.targets {
            return Err(config("prepared File API context changed"));
        }
        for target in &stored.targets {
            if let Some(artifacts) = target["artifacts"].as_array() {
                for artifact in artifacts {
                    if !artifact.as_str().is_some_and(|path| Path::new(path).is_file()) {
                        return Err(config("prepared build artifact missing"));
                    }
                }
            }
        }
        state["data"]["reused_build"] = json!(true);
        stored.source_excluded_paths = excluded.clone();
        (stored, inputs, barrier)
    } else {
        let mut engine_proofs = crate::cmake_engine::previous(root, &presets.binary_dir)?;
        let mut building_state = json!({"schema":1,"project":root,"profile":action.profile(),"source_digest":source,"source_excluded_paths":excluded,"config_digest":configuration,"fingerprint":null,"status":"building","cmake_engine_proofs":engine_proofs});
        report::write(&presets.binary_dir.join(".cpp-infra-build-state.json"), &building_state)?;
        let mut inputs = crate::cmake_inputs::CmakeInputs::new(root, &presets, dependency.as_ref())?;
        let cmake_version = runner.run("cmake", &["--version".into()], root, cfg.configure_timeout_seconds, &[])?;
        let version = cmake_version
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("cmake version "))
            .ok_or_else(missing_config("unrecognized CMake version output"))?;
        let parts: Vec<u32> = version
            .split('.')
            .take(2)
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| config("unrecognized CMake version"))?;
        if parts.len() != 2 || parts[0] < 3 || (parts[0] == 3 && parts[1] < 25) {
            return Err(config("CMake 3.25 or newer is required"));
        }
        crate::cmake_engine::check_current(root, &presets.binary_dir, version, &engine_proofs)?;
        file_api::query(&presets.binary_dir)?;
        let configure_args = vec!["--preset".into(), profile.configure.clone()];
        let configured = runner.run("cmake", &configure_args, root, cfg.configure_timeout_seconds, &[]);
        if !matches!(&configured, Err(VerifyError::Io(_))) {
            engine_proofs = crate::cmake_engine::capture(&presets.binary_dir, version, &engine_proofs)?;
            building_state["cmake_engine_proofs"] = json!(engine_proofs);
            report::write(&presets.binary_dir.join(".cpp-infra-build-state.json"), &building_state)?;
        }
        configured?;
        let mut context = file_api::read(root, action.profile(), profile, &presets, &source, &configuration)?;
        context.source_excluded_paths = excluded.clone();
        let (_, actual_version, _) = file_api::configuration_files(&context)?;
        if actual_version != version {
            return Err(config("CMake version probe and actual engine identity disagree"));
        }
        inputs.observe(&context)?;
        inputs.validate()?;
        verify_unchanged(root, &source, &configuration, &excluded, &presets.input_files)?;
        if let Some(dependency) = &dependency {
            verify_toolchain(&context, dependency)?;
        }
        state["external_versions"] = json!({"cmake":file_api::json_file(&context.file_api_index)?["cmake"]["version"],"compiler":context.compiler,"generator":context.generator});
        runner.run(
            "cmake",
            &[
                "--build".into(),
                "--preset".into(),
                profile.build.clone(),
                "--parallel".into(),
                cfg.jobs.to_string(),
            ],
            root,
            cfg.build_timeout_seconds,
            &[],
        )?;
        verify_unchanged(root, &source, &configuration, &excluded, &presets.input_files)?;
        // Re-read after build because CMake may regenerate its File API replies.
        context = file_api::read(root, action.profile(), profile, &presets, &source, &configuration)?;
        context.source_excluded_paths = excluded.clone();
        inputs.observe(&context)?;
        inputs.validate()?;
        context.configuration_inputs = inputs.records();
        attach_dependency(&mut context, dependency.clone())?;
        attach_configuration(&mut context)?;
        report::write(
            &presets.binary_dir.join(".cpp-infra-build-state.json"),
            &json!({"schema":1,"project":root,"profile":action.profile(),"source_digest":source,"source_excluded_paths":excluded,"config_digest":configuration,"fingerprint":context.fingerprint,"configuration_inputs":context.configuration_inputs,"cmake_cache_digest":context.cmake_cache_digest,"cmake_engine_proofs":engine_proofs,"status":"built"}),
        )?;
        let barrier = crate::build_barrier::BuildBarrier::capture(&context)?;
        (context, inputs, barrier)
    };
    state["external_versions"] = json!({"cmake":file_api::json_file(&context.file_api_index)?["cmake"]["version"],"compiler":context.compiler,"generator":context.generator});
    if presets.generator.contains("Ninja") {
        let version = runner.run("ninja", &["--version".into()], root, cfg.configure_timeout_seconds, &[])?;
        state["external_versions"]["ninja"] = json!(version.trim());
    }
    let context_path = runner.work_root.join("build-context.json");
    report::write(&context_path, &context)?;
    state["artifacts"]
        .as_array_mut()
        .ok_or_else(missing_config("artifacts invariant"))?
        .push(report::artifact(&runner.report_root, &context_path, "build-context")?);
    if let Action::Prepare {
        output: Some(output), ..
    } = action
        && output != &context_path
    {
        crate::context_output::validate(root, output)?;
        report::write(output, &context)?;
    }
    state["data"]["build_context"] = json!(context_path);
    let mut suites = Vec::new();
    for (index, suite) in selected.iter().enumerate() {
        let result = match suite.as_str() {
            "build" => Ok(json!({"targets":context.targets.len()})),
            "test" => crate::ctest::run(root, &context, &cfg, runner, &[]),
            "headers" => crate::headers::run(
                root,
                &context,
                cfg.headers
                    .as_ref()
                    .ok_or_else(missing_config("headers configuration missing"))?,
                &cfg,
                runner,
            ),
            "install" => crate::install::run(
                root,
                &context,
                cfg.install
                    .as_ref()
                    .ok_or_else(missing_config("install configuration missing"))?,
                &cfg,
                runner,
            ),
            "package" => crate::package::run(
                root,
                &context,
                cfg.package
                    .as_ref()
                    .ok_or_else(missing_config("package configuration missing"))?,
                &cfg,
                runner,
            ),
            "sanitize" => crate::sanitize::run(
                root,
                &context,
                cfg.sanitize
                    .as_ref()
                    .ok_or_else(missing_config("sanitize configuration missing"))?,
                &cfg,
                runner,
            ),
            "fuzz" => crate::fuzz::run(
                root,
                &context,
                cfg.fuzz
                    .as_ref()
                    .ok_or_else(missing_config("fuzz configuration missing"))?,
                &cfg,
                runner,
            ),
            _ => Err(config("unknown suite")),
        };
        let result = result.and_then(|data| {
            verify_consumption(&context, &cmake_inputs, &presets.input_files, &barrier)?;
            Ok(data)
        });
        match result {
            Ok(data) => suites.push(json!({"suite":suite,"status":"passed","data":data})),
            Err(error) => {
                suites.push(json!({"suite":suite,"status":"failed","message":error.to_string()}));
                for blocked in selected.iter().skip(index + 1) {
                    suites.push(json!({"suite":blocked,"status":"blocked"}));
                }
                state["data"]["suites"] = json!(suites);
                return Err(error);
            }
        }
    }
    state["data"]["suites"] = json!(suites);
    verify_consumption(&context, &cmake_inputs, &presets.input_files, &barrier)
}
fn verify_consumption(
    context: &crate::BuildContext,
    inputs: &crate::cmake_inputs::CmakeInputs,
    preset_inputs: &[PathBuf],
    barrier: &crate::build_barrier::BuildBarrier,
) -> Result<()> {
    barrier.validate(context)?;
    inputs.validate()?;
    verify_unchanged(
        &context.project,
        &context.source_digest,
        &context.config_digest,
        &context.source_excluded_paths,
        preset_inputs,
    )?;
    if let Some(dependency) = &context.dependency_context {
        verify_dependency(&context.project, dependency)?;
    }
    Ok(())
}
fn verify_dependency(root: &Path, value: &Value) -> Result<()> {
    let directory = Path::new(
        value["generators_dir"]
            .as_str()
            .ok_or_else(missing_config("dependency generators_dir missing"))?,
    )
    .canonicalize()?;
    if !directory.starts_with(root.join("build/infra")) {
        return Err(config("dependency generators outside build/infra"));
    }
    for generated in value["generated_files"]
        .as_array()
        .ok_or_else(missing_config("dependency generated_files missing"))?
    {
        let path = Path::new(
            generated["path"]
                .as_str()
                .ok_or_else(missing_config("dependency generated file missing path"))?,
        );
        if !path.canonicalize()?.starts_with(&directory) || digest::file(path)? != generated["sha256"] {
            return Err(config("dependency generated file fingerprint mismatch"));
        }
    }
    Ok(())
}
fn verify_unchanged(
    root: &Path,
    source: &str,
    configuration: &str,
    excluded: &[String],
    inputs: &[PathBuf],
) -> Result<()> {
    for path in [
        root.join(".infra/project.toml"),
        root.join(".infra/verify/config.toml"),
        root.join("CMakeLists.txt"),
    ]
    .iter()
    .chain(inputs)
    {
        crate::project_input::validate(root, path)
            .map_err(|error| config(format!("project configuration input became invalid: {error}")))?;
    }
    if digest::project(root, false, excluded)? != source || digest::project(root, true, &[])? != configuration {
        return Err(config("project changed during verification; refusing stale context"));
    }
    Ok(())
}
fn collect_artifacts(state: &mut Value, runner: &Runner) -> Result<()> {
    if !runner.work_root.is_dir() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(&runner.work_root).follow_links(false) {
        let entry = entry.map_err(protocol_error)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let kind = if entry.file_name() == "ctest-junit.xml" {
            Some("junit")
        } else if path.starts_with(runner.work_root.join("logs")) {
            Some("log")
        } else if entry.file_name() == "reproduction.json" {
            Some("reproduction")
        } else if path.components().any(|part| part.as_os_str() == "crashes") {
            Some("fuzz-crash")
        } else if path.parent() == Some(runner.work_root.join("package/archives").as_path())
            && (path
                .extension()
                .is_some_and(|extension| extension == "zip" || extension == "tgz")
                || path.to_string_lossy().ends_with(".tar.gz"))
        {
            Some("package")
        } else if entry.file_name() == "dependency-context.json" {
            Some("dependency-context")
        } else {
            None
        };
        if let Some(kind) = kind {
            let artifact = report::artifact(&runner.report_root, path, kind)?;
            let artifacts = state["artifacts"]
                .as_array_mut()
                .ok_or_else(missing_config("artifacts invariant"))?;
            if !artifacts.iter().any(|existing| existing["path"] == artifact["path"]) {
                artifacts.push(artifact);
            }
        }
    }
    Ok(())
}
fn verify_toolchain(context: &crate::BuildContext, dependency: &Value) -> Result<()> {
    let settings = dependency["settings"]
        .as_object()
        .ok_or_else(missing_config("dependency context settings missing"))?;
    let get = |name: &str| settings.get(name).and_then(Value::as_str);
    if let Some(expected) = get("compiler") {
        let actual = context.compiler["id"].as_str().unwrap_or("");
        let matches = match expected {
            "gcc" => actual == "GNU",
            "clang" => actual == "Clang",
            "apple-clang" => actual == "AppleClang",
            "msvc" => actual == "MSVC",
            _ => false,
        };
        if !matches {
            return Err(config("configured CMake compiler does not match Conan compiler"));
        }
    }
    if let Some(expected) = get("compiler.version") {
        let actual = context.compiler["version"].as_str().unwrap_or("");
        if actual != expected && !actual.starts_with(&format!("{expected}.")) {
            return Err(config(
                "configured CMake compiler version does not match Conan compiler.version",
            ));
        }
    }
    if let Some(expected) = get("compiler.cppstd")
        && context.compiler["cppstd"].as_str() != Some(expected.trim_start_matches("gnu"))
    {
        return Err(config(
            "configured CMake C++ standard does not match Conan compiler.cppstd",
        ));
    }
    if let Some(expected) = get("build_type")
        && context.compiler["build_type"].as_str() != Some(expected)
    {
        return Err(config("configured CMake build_type does not match Conan build_type"));
    }
    if let Some(expected) = get("compiler.runtime") {
        let actual = context.compiler["runtime"]
            .as_str()
            .ok_or_else(missing_config("cannot verify CMake MSVC runtime against Conan"))?;
        let matches = match expected {
            "dynamic" => actual.contains("DLL"),
            "static" => !actual.contains("DLL"),
            _ => false,
        };
        if !matches {
            return Err(config("configured MSVC runtime does not match Conan compiler.runtime"));
        }
    }
    Ok(())
}

fn attach_dependency(context: &mut crate::BuildContext, dependency: Option<Value>) -> Result<()> {
    if let Some(value) = &dependency {
        let fingerprint = value["fingerprint"]
            .as_str()
            .ok_or_else(missing_config("dependency fingerprint missing"))?;
        let metadata = digest::bytes(&serde_json::to_vec(value).map_err(protocol_error)?);
        context.fingerprint = digest::bytes(format!("{}:{fingerprint}:{metadata}", context.fingerprint).as_bytes());
    }
    context.dependency_context = dependency;
    Ok(())
}

fn attach_configuration(context: &mut crate::BuildContext) -> Result<()> {
    let proof = digest::bytes(&serde_json::to_vec(&context.configuration_inputs).map_err(protocol_error)?);
    context.fingerprint = digest::bytes(format!("{}:{proof}", context.fingerprint).as_bytes());
    Ok(())
}
