// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Resolve committed CMake presets without environment-dependent build paths.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;
use serde_json::json;

use crate::config::Profile;
use crate::config::canonical_future;
use crate::config::relative_path;
use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::missing_config;
#[derive(Debug)]
pub(crate) struct Presets {
    pub binary_dir: PathBuf,
    pub generator: String,
    pub configuration: Option<String>,
    pub configure: Value,
    pub input_files: Vec<PathBuf>,
}
fn read(
    path: &Path,
    root: &Path,
    stack: &mut BTreeSet<PathBuf>,
    all: &mut BTreeMap<String, BTreeMap<String, Value>>,
) -> Result<()> {
    read_includes(path, root, stack, &mut BTreeSet::new(), all)
}
fn read_includes(
    path: &Path,
    root: &Path,
    stack: &mut BTreeSet<PathBuf>,
    visited: &mut BTreeSet<PathBuf>,
    all: &mut BTreeMap<String, BTreeMap<String, Value>>,
) -> Result<()> {
    crate::project_input::validate(root, path)?;
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root) || !stack.insert(canonical.clone()) {
        return Err(config("preset include escapes project or has a cycle"));
    }
    if visited.contains(&canonical) {
        stack.remove(&canonical);
        return Ok(());
    }
    let value: Value = serde_json::from_slice(&fs::read(&canonical)?).map_err(|e| config(format!("presets: {e}")))?;
    if value["version"] != 6 {
        return Err(config("only CMake Presets schema 6 is supported"));
    }
    if let Some(includes) = value["include"].as_array() {
        for item in includes {
            let name = item
                .as_str()
                .ok_or_else(missing_config("preset include must be a string"))?;
            let included = canonical
                .parent()
                .ok_or_else(missing_config("preset parent"))?
                .join(name);
            read_includes(&included, root, stack, visited, all)?;
        }
    }
    for kind in ["configurePresets", "buildPresets", "testPresets", "packagePresets"] {
        if let Some(items) = value[kind].as_array() {
            for item in items {
                let name = item["name"]
                    .as_str()
                    .ok_or_else(missing_config("preset missing name"))?;
                if all
                    .entry(kind.into())
                    .or_default()
                    .insert(
                        name.into(),
                        with_file_dir(
                            item,
                            canonical
                                .parent()
                                .ok_or_else(missing_config("preset file parent missing"))?,
                        ),
                    )
                    .is_some()
                {
                    return Err(config(format!("duplicate preset {name}")));
                }
            }
        }
    }
    stack.remove(&canonical);
    visited.insert(canonical);
    Ok(())
}
fn resolve(
    kind: &str,
    name: &str,
    all: &BTreeMap<String, BTreeMap<String, Value>>,
    stack: &mut BTreeSet<String>,
) -> Result<Value> {
    if !stack.insert(name.into()) {
        return Err(config("preset inheritance cycle"));
    }
    let item = all
        .get(kind)
        .and_then(|items| items.get(name))
        .ok_or_else(|| config(format!("missing {kind} preset: {name}")))?;
    let mut result = json!({});
    let parents: Vec<String> = match &item["inherits"] {
        Value::String(name) => vec![name.clone()],
        Value::Array(items) => items
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(missing_config("bad inherits")))
            .collect::<Result<_>>()?,
        Value::Null => Vec::new(),
        _ => return Err(config("bad inherits")),
    };
    for parent in parents.iter().rev() {
        let mut inherited = resolve(kind, parent, all, stack)?;
        if let Some(values) = inherited.as_object_mut() {
            for key in ["name", "hidden", "inherits", "description", "displayName"] {
                values.remove(key);
            }
        }
        merge(&mut result, &inherited);
    }
    merge(&mut result, item);
    stack.remove(name);
    Ok(result)
}
fn merge(left: &mut Value, right: &Value) {
    if let (Some(left), Some(right)) = (left.as_object_mut(), right.as_object()) {
        for (key, value) in right {
            if value.is_object() && left.get(key).is_some_and(Value::is_object) {
                if let Some(existing) = left.get_mut(key) {
                    merge(existing, value);
                }
            } else {
                left.insert(key.clone(), value.clone());
            }
        }
    }
}
pub(crate) fn load(root: &Path, profile: &Profile) -> Result<Presets> {
    let mut all = BTreeMap::new();
    let mut visited = BTreeSet::new();
    read_includes(
        &root.join("CMakePresets.json"),
        root,
        &mut BTreeSet::new(),
        &mut visited,
        &mut all,
    )?;
    if root.join("CMakeUserPresets.json").exists() {
        let mut with_user = all.clone();
        read_includes(
            &root.join("CMakeUserPresets.json"),
            root,
            &mut BTreeSet::new(),
            &mut visited,
            &mut with_user,
        )
        .map_err(|error| {
            config(format!(
                "local user preset conflicts with reproducible project configuration: {error}"
            ))
        })?;
    }
    let configure = resolve("configurePresets", &profile.configure, &all, &mut BTreeSet::new())?;
    let build = resolve("buildPresets", &profile.build, &all, &mut BTreeSet::new())?;
    let test = profile
        .test
        .as_ref()
        .map(|name| resolve("testPresets", name, &all, &mut BTreeSet::new()))
        .transpose()?;
    if build["configurePreset"] != profile.configure
        || test
            .as_ref()
            .is_some_and(|test| test["configurePreset"] != profile.configure)
    {
        return Err(config(
            "profile build/test preset does not refer to its configure preset",
        ));
    }
    if configure["hidden"] == true || build["hidden"] == true {
        return Err(config("profile cannot select hidden preset"));
    }
    let binary = configure["binaryDir"]
        .as_str()
        .ok_or_else(missing_config("configure preset requires binaryDir"))?
        .replace(
            "${sourceDir}",
            root.to_str().ok_or_else(missing_config("non UTF-8 project"))?,
        )
        .replace("${presetName}", &profile.configure);
    if binary.contains('$') {
        return Err(config("binaryDir contains unsupported macro"));
    }
    let binary = Path::new(&binary);
    let relative = if binary.is_absolute() {
        binary
            .strip_prefix(root)
            .map_err(|_| config("binaryDir outside project"))?
    } else {
        binary
    };
    let binary_dir = relative_path(
        root,
        relative.to_str().ok_or_else(missing_config("non UTF-8 binary dir"))?,
        false,
    )?;
    let managed = root.join("build/infra");
    if !binary_dir.starts_with(&managed) || binary_dir == managed {
        return Err(config("binaryDir must have a dedicated directory under build/infra"));
    }
    let actual_binary = canonical_future(&binary_dir)?;
    let actual_managed = canonical_future(&managed)?;
    if actual_managed != managed
        || actual_binary != binary_dir
        || !actual_binary.starts_with(&actual_managed)
        || actual_binary == actual_managed
    {
        return Err(config("binaryDir symlink escapes managed build root"));
    }
    let generator = configure["generator"]
        .as_str()
        .ok_or_else(missing_config("preset generator must be explicit"))?
        .to_owned();
    let configuration = build["configuration"].as_str().map(str::to_owned);
    if (generator.contains("Multi-Config") || generator.contains("Visual Studio") || generator == "Xcode")
        && configuration.is_none()
    {
        return Err(config("multi-config generator requires build configuration"));
    }
    if let Some(test) = &test
        && test["configuration"]
            .as_str()
            .is_some_and(|value| Some(value) != configuration.as_deref())
    {
        return Err(config("build/test configuration mismatch"));
    }
    Ok(Presets {
        binary_dir,
        generator,
        configuration,
        configure,
        input_files: visited.into_iter().collect(),
    })
}

pub(crate) fn validate_package(root: &Path, name: &str, configure: &str) -> Result<()> {
    let mut all = BTreeMap::new();
    read(&root.join("CMakePresets.json"), root, &mut BTreeSet::new(), &mut all)?;
    let preset = resolve("packagePresets", name, &all, &mut BTreeSet::new())?;
    if preset["configurePreset"] != configure || preset["hidden"] == true {
        return Err(config("package preset must select the requested configure preset"));
    }
    let generators = preset["generators"].as_array().ok_or_else(missing_config(
        "package preset must explicitly select ZIP/TGZ generators",
    ))?;
    if generators.is_empty()
        || generators
            .iter()
            .any(|generator| !matches!(generator.as_str(), Some("ZIP" | "TGZ")))
    {
        return Err(config("first release supports only ZIP and TGZ package generators"));
    }
    Ok(())
}

fn with_file_dir(value: &Value, directory: &Path) -> Value {
    let mut value = value.clone();
    for name in ["toolchainFile", "cacheVariables", "environment"] {
        if let Some(value) = value.get_mut(name) {
            replace_file_dir(value, &directory.to_string_lossy());
        }
    }
    value
}
fn replace_file_dir(value: &mut Value, directory: &str) {
    match value {
        Value::String(value) => *value = value.replace("${fileDir}", directory),
        Value::Object(values) => {
            for value in values.values_mut() {
                replace_file_dir(value, directory);
            }
        }
        _ => {}
    }
}
