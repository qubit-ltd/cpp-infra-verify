// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Discover project configuration inputs and bind generated inputs to their
//! owner.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;
use serde_json::json;

use crate::digest;
use crate::presets::Presets;
use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::missing_config;

pub(crate) struct CmakeInputs {
    root: PathBuf,
    binary: PathBuf,
    variables: BTreeMap<String, String>,
    environment: BTreeMap<String, Value>,
    approved: BTreeMap<PathBuf, String>,
    records: BTreeMap<PathBuf, Value>,
    visited: BTreeSet<PathBuf>,
    cmake_root: Option<PathBuf>,
    cache_digest: Option<String>,
}
fn config_key(name: &str) -> bool {
    name == "CMAKE_TOOLCHAIN_FILE"
        || name == "CMAKE_PROJECT_TOP_LEVEL_INCLUDES"
        || name.starts_with("CMAKE_USER_MAKE_RULES_OVERRIDE")
        || name.starts_with("CMAKE_PROJECT_") && (name.ends_with("_INCLUDE") || name.ends_with("_INCLUDE_BEFORE"))
}
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(if *value { "ON" } else { "OFF" }.into()),
        Value::Object(value) => value.get("value").and_then(scalar),
        _ => None,
    }
}
impl CmakeInputs {
    pub(crate) fn new(root: &Path, presets: &Presets, dependency: Option<&Value>) -> Result<Self> {
        let mut input = Self {
            root: root.into(),
            binary: presets.binary_dir.clone(),
            variables: BTreeMap::new(),
            environment: presets.configure["environment"]
                .as_object()
                .map(|value| value.iter().map(|(key, value)| (key.clone(), value.clone())).collect())
                .unwrap_or_default(),
            approved: BTreeMap::new(),
            records: BTreeMap::new(),
            visited: BTreeSet::new(),
            cmake_root: None,
            cache_digest: None,
        };
        if let Some(dependency) = dependency {
            if !dependency["fingerprint"]
                .as_str()
                .is_some_and(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            {
                return Err(config("dependency configuration proof fingerprint missing or invalid"));
            }
            let directory = PathBuf::from(
                dependency["generators_dir"]
                    .as_str()
                    .ok_or_else(missing_config("dependency generators_dir missing"))?,
            );
            if directory.canonicalize()? != directory || !directory.starts_with(&input.binary) {
                return Err(config(
                    "dependency generated configuration must belong to this binary directory",
                ));
            }
            for file in dependency["generated_files"]
                .as_array()
                .ok_or_else(missing_config("dependency generated_files missing"))?
            {
                let path = PathBuf::from(
                    file["path"]
                        .as_str()
                        .ok_or_else(missing_config("generated configuration path missing"))?,
                );
                let hash = file["sha256"]
                    .as_str()
                    .ok_or_else(missing_config("generated configuration hash missing"))?;
                if !path.starts_with(&directory)
                    || path.canonicalize()? != path
                    || !path.is_file()
                    || digest::file(&path)? != hash
                {
                    return Err(config("dependency generated configuration hash/ownership mismatch"));
                }
                input.approved.insert(path, hash.into());
            }
        }
        let cache = presets.binary_dir.join("CMakeCache.txt");
        if cache.is_file() {
            for line in fs::read_to_string(cache)?.lines() {
                if line.starts_with(['#', '/']) {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=')
                    && let Some((name, _)) = key.split_once(':')
                {
                    input.variables.insert(name.into(), value.into());
                }
            }
        }
        if let Some(values) = presets.configure["cacheVariables"].as_object() {
            for (name, value) in values {
                if let Some(value) = scalar(value) {
                    input.variables.insert(name.clone(), value);
                } else if value.is_null() {
                    input.variables.remove(name);
                }
            }
        }
        if let Some(toolchain) = presets.configure["toolchainFile"].as_str() {
            input.variables.insert("CMAKE_TOOLCHAIN_FILE".into(), toolchain.into());
        }
        if !input.variables.contains_key("CMAKE_TOOLCHAIN_FILE")
            && let Ok(value) = std::env::var("CMAKE_TOOLCHAIN_FILE")
        {
            input.variables.insert("CMAKE_TOOLCHAIN_FILE".into(), value);
        }
        for (name, value) in [
            ("sourceDir", root.to_string_lossy().into_owned()),
            (
                "sourceParentDir",
                root.parent().unwrap_or(root).to_string_lossy().into_owned(),
            ),
            (
                "sourceDirName",
                root.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            ),
            ("presetName", presets.configure["name"].as_str().unwrap_or("").into()),
            ("generator", presets.generator.clone()),
            ("CMAKE_SOURCE_DIR", root.to_string_lossy().into_owned()),
            ("PROJECT_SOURCE_DIR", root.to_string_lossy().into_owned()),
            ("CMAKE_BINARY_DIR", presets.binary_dir.to_string_lossy().into_owned()),
            ("PROJECT_BINARY_DIR", presets.binary_dir.to_string_lossy().into_owned()),
        ] {
            input.variables.insert(name.into(), value);
        }
        let selected: Vec<(String, String)> = input
            .variables
            .iter()
            .filter(|(name, _)| config_key(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        for (key, value) in selected {
            if let Some(value) = input.expand(&value, root, root, 0)? {
                for name in value.split(';').filter(|name| !name.is_empty()) {
                    let path = if key == "CMAKE_TOOLCHAIN_FILE"
                        && !Path::new(name).is_absolute()
                        && input.binary.join(name).exists()
                    {
                        input.binary.join(name)
                    } else {
                        input.resolve_path(name, root)
                    };
                    input.scan(&path, root)?;
                }
            }
        }
        input.scan(&root.join("CMakeLists.txt"), root)?;
        Ok(input)
    }
    fn expand(&self, text: &str, list_dir: &Path, source: &Path, depth: usize) -> Result<Option<String>> {
        if depth > 32 {
            return Err(config("CMake configuration variable expansion cycle"));
        }
        let mut output = text.to_owned();
        for prefix in ["${", "$env{", "$penv{"] {
            while let Some(start) = output.find(prefix) {
                let end = start
                    + prefix.len()
                    + output[start + prefix.len()..]
                        .find('}')
                        .ok_or_else(missing_config("unterminated configuration macro"))?;
                let name = &output[start + prefix.len()..end];
                let value = if prefix == "${" {
                    match name {
                        "CMAKE_CURRENT_LIST_DIR" => Some(list_dir.to_string_lossy().into_owned()),
                        "CMAKE_CURRENT_SOURCE_DIR" => Some(source.to_string_lossy().into_owned()),
                        "CMAKE_CURRENT_BINARY_DIR" => Some(self.binary.to_string_lossy().into_owned()),
                        _ => self.variables.get(name).cloned(),
                    }
                } else if prefix == "$penv{" {
                    std::env::var(name).ok()
                } else if let Some(value) = self.environment.get(name) {
                    if value.is_null() {
                        Some(String::new())
                    } else {
                        value.as_str().map(str::to_owned)
                    }
                } else {
                    std::env::var(name).ok()
                };
                let Some(value) = value else {
                    return Ok(None);
                };
                let Some(value) = self.expand(&value, list_dir, source, depth + 1)? else {
                    return Ok(None);
                };
                output.replace_range(start..=end, &value);
            }
        }
        if output.contains('$') {
            return Ok(None);
        }
        Ok(Some(output))
    }
    fn resolve_path(&self, name: &str, base: &Path) -> PathBuf {
        let path = Path::new(name);
        if path.is_absolute() {
            path.into()
        } else {
            base.join(path)
        }
    }
    fn add(&mut self, path: &Path) -> Result<()> {
        crate::project_input::reject_symlinks(&self.root, path)?;
        let canonical = path.canonicalize()?;
        let kind = if let Some(expected) = self.approved.get(&canonical) {
            if digest::file(path)? != *expected {
                return Err(config("dependency configuration changed after prepare"));
            }
            "dependency-generated"
        } else {
            crate::project_input::validate(&self.root, path)?;
            "project"
        };
        let canonical = path.canonicalize()?;
        let hash = digest::file(path)?;
        if self
            .records
            .get(&canonical)
            .is_some_and(|record| record["sha256"] != hash)
        {
            return Err(config("CMake configuration input changed during verification"));
        }
        self.records
            .insert(canonical.clone(), json!({"path":canonical,"sha256":hash,"kind":kind}));
        Ok(())
    }
    fn scan(&mut self, path: &Path, source: &Path) -> Result<()> {
        crate::project_input::reject_symlinks(&self.root, path)?;
        if !path.canonicalize()?.starts_with(&self.root) {
            return Ok(());
        }
        self.add(path)?;
        let actual = path.canonicalize()?;
        if !self.visited.insert(actual.clone()) || self.approved.contains_key(&actual) {
            return Ok(());
        }
        let list = actual
            .parent()
            .ok_or_else(missing_config("CMake input parent missing"))?;
        let mut conditional = 0usize;
        let mut definition = 0usize;
        for (name, args) in commands(&fs::read_to_string(&actual)?) {
            match name.as_str() {
                "function" | "macro" => {
                    definition += 1;
                    continue;
                }
                "endfunction" | "endmacro" => {
                    definition = definition.saturating_sub(1);
                    continue;
                }
                "if" => {
                    conditional += 1;
                    continue;
                }
                "endif" => {
                    conditional = conditional.saturating_sub(1);
                    continue;
                }
                _ => {}
            }
            if conditional > 0 || definition > 0 || args.is_empty() {
                continue;
            }
            if name == "set" {
                let values = args
                    .iter()
                    .skip(1)
                    .take_while(|value| value.as_str() != "CACHE" && value.as_str() != "PARENT_SCOPE")
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(";");
                self.variables.insert(args[0].clone(), values);
                continue;
            }
            if name == "list" && args.len() > 2 && matches!(args[0].as_str(), "APPEND" | "PREPEND") {
                let more = args[2..].join(";");
                let entry = self.variables.entry(args[1].clone()).or_default();
                if args[0] == "APPEND" {
                    if !entry.is_empty() {
                        entry.push(';');
                    }
                    entry.push_str(&more);
                } else {
                    *entry = format!("{more};{entry}");
                }
                continue;
            }
            if !matches!(name.as_str(), "include" | "add_subdirectory") {
                continue;
            }
            let Some(value) = self.expand(&args[0], list, source, 0)? else {
                continue;
            };
            let mut candidate = self.resolve_path(&value, source);
            if name == "include" && !value.contains(['/', '\\']) && !value.ends_with(".cmake") {
                let mut found = None;
                if let Some(paths) = self.variables.get("CMAKE_MODULE_PATH")
                    && let Some(paths) = self.expand(paths, list, source, 0)?
                {
                    for directory in paths.split(';') {
                        let path = self.resolve_path(directory, source).join(format!("{value}.cmake"));
                        if path.exists() {
                            found = Some(path);
                            break;
                        }
                    }
                }
                let Some(path) = found else {
                    continue;
                };
                candidate = path;
            }
            if name == "add_subdirectory" {
                candidate = candidate.join("CMakeLists.txt");
            }
            if !candidate.exists() {
                continue;
            }
            let next_source = if name == "add_subdirectory" {
                candidate.parent().unwrap_or(source)
            } else {
                source
            };
            self.scan(&candidate, next_source)?;
        }
        Ok(())
    }
    pub(crate) fn observe(&mut self, context: &crate::BuildContext) -> Result<()> {
        if self
            .cache_digest
            .as_ref()
            .is_some_and(|expected| *expected != context.cmake_cache_digest)
        {
            return Err(config("CMake cache changed during configuration consumption"));
        }
        self.cache_digest = Some(context.cmake_cache_digest.clone());
        let (cmake_root, version, inputs) = crate::file_api::configuration_files(context)?;
        self.cmake_root = Some(cmake_root.clone());
        for item in inputs {
            let name = item["path"]
                .as_str()
                .ok_or_else(missing_config("File API CMake input path missing"))?;
            let path = self.resolve_path(name, &self.root);
            let actual = path.canonicalize()?;
            if actual.starts_with(&cmake_root) {
                crate::project_input::reject_symlinks(&self.root, &path)?;
                self.artifact(&actual, "cmake-tool")?;
                continue;
            }
            if item["isExternal"] == true {
                continue;
            }
            let internal = item["isGenerated"] == true && crate::cmake_engine::owns(&self.binary, &version, &actual);
            if internal {
                self.artifact(&path, "cmake-generated")?;
                continue;
            }
            self.add(&path)?;
        }
        Ok(())
    }
    fn artifact(&mut self, path: &Path, kind: &str) -> Result<()> {
        if path.canonicalize()? != path || !path.is_file() {
            return Err(config("CMake configuration artifact cannot be a symlink"));
        }
        let hash = digest::file(path)?;
        if self.records.get(path).is_some_and(|record| record["sha256"] != hash) {
            return Err(config("CMake configuration artifact changed during verification"));
        }
        self.records
            .insert(path.into(), json!({"path":path,"sha256":hash,"kind":kind}));
        Ok(())
    }
    pub(crate) fn records(&self) -> Vec<Value> {
        self.records.values().cloned().collect()
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if let Some(expected) = &self.cache_digest
            && digest::file(&self.binary.join("CMakeCache.txt"))? != *expected
        {
            return Err(config("CMake cache changed during configuration consumption"));
        }
        for record in self.records.values() {
            let path = Path::new(
                record["path"]
                    .as_str()
                    .ok_or_else(missing_config("CMake input record missing path"))?,
            );
            match record["kind"].as_str() {
                Some("project") => crate::project_input::validate(&self.root, path)?,
                Some("dependency-generated") => {
                    if self.approved.get(path) != record["sha256"].as_str().map(str::to_owned).as_ref() {
                        return Err(config("generated CMake input ownership proof changed"));
                    }
                }
                Some("cmake-generated") => {
                    if !path.starts_with(self.binary.join("CMakeFiles")) {
                        return Err(config("CMake generated configuration artifact escapes binary owner"));
                    }
                }
                Some("cmake-tool") => {
                    if !self.cmake_root.as_ref().is_some_and(|root| path.starts_with(root)) {
                        return Err(config("CMake tool configuration artifact escapes installation owner"));
                    }
                }
                _ => return Err(config("unknown CMake configuration input role")),
            }
            if path.canonicalize()? != path || digest::file(path)? != record["sha256"] {
                return Err(config("CMake configuration input changed"));
            }
        }
        for (path, expected) in &self.approved {
            if path.canonicalize()? != *path || digest::file(path)? != *expected {
                return Err(config("dependency generated input proof changed"));
            }
        }
        Ok(())
    }
}

// Only identify command boundaries and literal arguments. CMake remains the
// interpreter; its File API supplies the authoritative actually-read file list.
fn commands(text: &str) -> Vec<(String, Vec<String>)> {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    let mut result = Vec::new();
    while index < bytes.len() {
        skip(bytes, &mut index);
        if index == bytes.len() {
            break;
        }
        let start = index;
        while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_') {
            index += 1;
        }
        if start == index {
            index += 1;
            continue;
        }
        let name = text[start..index].to_ascii_lowercase();
        skip(bytes, &mut index);
        if bytes.get(index) != Some(&b'(') {
            continue;
        }
        index += 1;
        let mut depth = 1usize;
        let mut args = Vec::new();
        while index < bytes.len() && depth > 0 {
            skip(bytes, &mut index);
            if index == bytes.len() {
                break;
            }
            match bytes[index] {
                b')' => {
                    depth -= 1;
                    index += 1;
                }
                b'(' => {
                    depth += 1;
                    index += 1;
                }
                b'"' => {
                    index += 1;
                    let start = index;
                    let mut escaped = false;
                    while index < bytes.len() {
                        if !escaped && bytes[index] == b'"' {
                            break;
                        }
                        escaped = !escaped && bytes[index] == b'\\';
                        if bytes[index] != b'\\' {
                            escaped = false;
                        }
                        index += 1;
                    }
                    args.push(text[start..index].to_owned());
                    if index < bytes.len() {
                        index += 1;
                    }
                }
                b'[' => {
                    if let Some((start, end, next)) = bracket(bytes, index) {
                        args.push(text[start..end].to_owned());
                        index = next;
                    } else {
                        let start = index;
                        index += 1;
                        while index < bytes.len()
                            && !bytes[index].is_ascii_whitespace()
                            && !matches!(bytes[index], b'(' | b')')
                        {
                            if bytes[index] == b'\\' && index + 1 < bytes.len() {
                                index += 2;
                            } else {
                                index += 1;
                            }
                        }
                        args.push(text[start..index].into());
                    }
                }
                _ => {
                    let start = index;
                    while index < bytes.len()
                        && !bytes[index].is_ascii_whitespace()
                        && !matches!(bytes[index], b'(' | b')')
                    {
                        if bytes[index] == b'\\' && index + 1 < bytes.len() {
                            index += 2;
                        } else {
                            index += 1;
                        }
                    }
                    args.push(text[start..index].into());
                }
            }
        }
        if depth != 0 {
            return result;
        }
        result.push((name, args));
    }
    result
}
fn bracket(bytes: &[u8], index: usize) -> Option<(usize, usize, usize)> {
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    let mut marker = index + 1;
    while bytes.get(marker) == Some(&b'=') {
        marker += 1;
    }
    if bytes.get(marker) != Some(&b'[') {
        return None;
    }
    let start = marker + 1;
    let count = marker - index - 1;
    let mut end = start;
    while end < bytes.len() {
        if bytes[end] == b']'
            && bytes
                .get(end + 1..end + 1 + count)
                .is_some_and(|value| value.iter().all(|byte| *byte == b'='))
            && bytes.get(end + 1 + count) == Some(&b']')
        {
            return Some((start, end, end + count + 2));
        }
        end += 1;
    }
    None
}
fn skip(bytes: &[u8], index: &mut usize) {
    loop {
        while *index < bytes.len() && bytes[*index].is_ascii_whitespace() {
            *index += 1;
        }
        if bytes.get(*index) != Some(&b'#') {
            break;
        }
        if let Some((_, _, next)) = bracket(bytes, *index + 1) {
            *index = next;
        } else {
            while *index < bytes.len() && bytes[*index] != b'\n' {
                *index += 1;
            }
        }
    }
}
