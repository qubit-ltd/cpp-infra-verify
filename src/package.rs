// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! CPack archive production and bounded, traversal-safe extraction.
use std::collections::BTreeSet;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use flate2::read::GzDecoder;
use serde_json::Value;
use serde_json::json;
use walkdir::WalkDir;

use crate::build_context::BuildContext;
use crate::config::Config;
use crate::config::Package;
use crate::config::relative_path;
use crate::runner::Runner;
use crate::verify_error::Result;
use crate::verify_error::config;
use crate::verify_error::gate;
use crate::verify_error::protocol_error;
const MAX_BYTES: u64 = 512 * 1024 * 1024;
fn safe_path(name: &Path, seen: &mut BTreeSet<PathBuf>) -> Result<PathBuf> {
    if name.is_absolute()
        || name.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(gate(format!("unsafe archive entry: {}", name.display())));
    }
    let normalized: PathBuf = name
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect();
    if normalized.as_os_str().is_empty() || !seen.insert(normalized.clone()) || seen.len() > 100_000 {
        return Err(gate("duplicate, empty, or excessive archive entry"));
    }
    Ok(normalized)
}
fn write_member(destination: &Path, reader: &mut impl Read, size: u64, budget: &mut u64) -> Result<()> {
    if size > *budget {
        return Err(gate("archive exceeds extraction size budget"));
    }
    *budget -= size;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = File::create(destination)?;
    let count = std::io::copy(&mut reader.take(size + 1), &mut output)?;
    if count != size {
        return Err(gate("archive member size mismatch"));
    }
    output.flush()?;
    Ok(())
}
fn extract(path: &Path, output: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    let mut seen = BTreeSet::new();
    let mut budget = MAX_BYTES;
    if path.extension().is_some_and(|ext| ext == "zip") {
        let mut archive = zip::ZipArchive::new(File::open(path)?).map_err(|e| gate(format!("ZIP: {e}")))?;
        if archive.has_duplicate_names() {
            return Err(gate("duplicate archive entry in ZIP central directory"));
        }
        for index in 0..archive.len() {
            let mut member = archive.by_index(index).map_err(|e| gate(format!("ZIP member: {e}")))?;
            let name = member
                .name()
                .map_err(|error| gate(format!("ZIP entry name: {error}")))?;
            let relative = safe_path(Path::new(name.as_ref()), &mut seen)?;
            if member.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) {
                return Err(gate("ZIP symlink entry is not supported"));
            }
            let destination = output.join(relative);
            if member.is_dir() {
                fs::create_dir_all(destination)?;
            } else {
                let size = member.size();
                write_member(&destination, &mut member, size, &mut budget)?;
            }
        }
    } else if path.to_string_lossy().ends_with(".tar.gz") || path.extension().is_some_and(|ext| ext == "tgz") {
        let mut archive = tar::Archive::new(GzDecoder::new(File::open(path)?));
        for member in archive.entries()? {
            let mut member = member?;
            let relative = safe_path(&member.path()?, &mut seen)?;
            let destination = output.join(relative);
            let kind = member.header().entry_type();
            if kind.is_dir() {
                fs::create_dir_all(destination)?;
            } else if kind.is_file() {
                let size = member.size();
                write_member(&destination, &mut member, size, &mut budget)?;
            } else {
                return Err(gate("TGZ contains unsupported symlink, link, or special entry"));
            }
        }
    } else {
        return Err(config("first release supports only ZIP and TGZ package formats"));
    }
    Ok(())
}
pub(crate) fn run(
    root: &Path,
    context: &BuildContext,
    package: &Package,
    cfg: &Config,
    runner: &mut Runner,
) -> Result<Value> {
    let output = runner.work_root.join("package/archives");
    fs::create_dir_all(&output)?;
    let mut args = vec![
        "--preset".into(),
        package.preset.clone(),
        "-B".into(),
        output.to_string_lossy().into_owned(),
    ];
    if let Some(configuration) = &context.configuration {
        args.extend(["-C".into(), configuration.clone()]);
    }
    runner.run("cpack", &args, root, cfg.build_timeout_seconds, &[])?;
    let mut archives: Vec<PathBuf> = fs::read_dir(&output)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<_>>()?;
    archives.retain(|path| {
        path.is_file()
            && (path.extension().is_some_and(|ext| ext == "zip" || ext == "tgz")
                || path.to_string_lossy().ends_with(".tar.gz"))
    });
    archives.sort();
    if archives.is_empty() {
        return Err(gate("CPack produced no supported ZIP/TGZ archive"));
    }
    if package.required_files.is_empty() {
        return Err(config("package required_files must not be empty"));
    }
    for (index, archive) in archives.iter().enumerate() {
        let unpacked = runner.work_root.join(format!("package/extracted-{index}"));
        extract(archive, &unpacked)?;
        for required in &package.required_files {
            relative_path(root, required, false)?;
            let found = WalkDir::new(&unpacked).into_iter().try_fold(false, |found, entry| {
                let entry = entry.map_err(protocol_error)?;
                Ok::<bool, crate::verify_error::VerifyError>(
                    found || (entry.file_type().is_file() && entry.path().ends_with(Path::new(required))),
                )
            })?;
            if !found {
                return Err(gate(format!("package missing required file: {required}")));
            }
        }
    }
    Ok(json!({"archives":archives,"required_files":package.required_files}))
}
#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use crate::package::safe_path;
    #[test]
    fn test_archive_traversal_and_duplicates_are_rejected() {
        let mut seen = BTreeSet::new();
        assert!(safe_path(Path::new("../escape"), &mut seen).is_err());
        assert!(safe_path(Path::new("/absolute"), &mut seen).is_err());
        assert!(safe_path(Path::new("include/header.h"), &mut seen).is_ok());
        assert!(safe_path(Path::new("include/./header.h"), &mut seen).is_err());
    }
    #[test]
    fn test_extract_rejects_traversal_and_duplicate_zip_entries() {
        use std::fs::File;
        use std::io::Write;

        use zip::write::SimpleFileOptions;
        for names in [["../escaped", "safe"], ["include/a.h", "include/./a.h"]] {
            let temp = tempfile::tempdir().expect("archive fixture");
            let archive = temp.path().join("package.zip");
            let mut zip = zip::ZipWriter::new(File::create(&archive).expect("archive"));
            for name in names {
                zip.start_file(name, SimpleFileOptions::default()).expect("member");
                zip.write_all(b"header").expect("content");
            }
            zip.finish().expect("finish");
            assert!(matches!(
                crate::package::extract(&archive, &temp.path().join("out")),
                Err(crate::VerifyError::Gate(_))
            ));
            assert!(!temp.path().join("escaped").exists());
        }
    }
    #[cfg(unix)]
    #[test]
    fn test_zip_symlinks_are_rejected_and_privileged_mode_bits_are_not_applied() {
        use std::fs;
        use std::fs::File;
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        use zip::write::SimpleFileOptions;
        let temp = tempfile::tempdir().expect("archive fixture");
        let outside = temp.path().join("outside");
        fs::write(&outside, b"untouched").expect("outside data");
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o640)).expect("outside mode");
        let archive = temp.path().join("symlink.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).expect("archive"));
        zip.add_symlink("link", "../outside", SimpleFileOptions::default())
            .expect("symlink entry");
        zip.finish().expect("finish");
        let error = crate::package::extract(&archive, &temp.path().join("symlink-out")).expect_err("reject link");
        assert_eq!(error.exit_code(), 1);
        assert!(error.to_string().contains("ZIP symlink"));
        assert_eq!(fs::read(&outside).expect("outside retained"), b"untouched");
        assert_eq!(
            fs::metadata(&outside).expect("outside mode").permissions().mode() & 0o7777,
            0o640
        );
        assert!(!temp.path().join("symlink-out/link").exists());
        let archive = temp.path().join("privileged.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).expect("archive"));
        zip.start_file(
            "regular",
            SimpleFileOptions::default().external_attributes(0o107777 << 16),
        )
        .expect("privileged metadata");
        zip.write_all(b"ordinary data").expect("content");
        zip.finish().expect("finish");
        let mut input = zip::ZipArchive::new(File::open(&archive).expect("archive")).expect("read archive");
        assert_eq!(
            (input.by_index(0).expect("member").external_attributes() >> 16) & 0o7000,
            0o7000
        );
        let output = temp.path().join("regular-out");
        crate::package::extract(&archive, &output).expect("extract regular file");
        assert_eq!(
            fs::read(output.join("regular")).expect("extracted content"),
            b"ordinary data"
        );
        assert_eq!(
            fs::metadata(output.join("regular"))
                .expect("safe mode")
                .permissions()
                .mode()
                & 0o7000,
            0
        );
    }
    #[test]
    fn test_zip_legacy_cp437_names_preserve_the_extracted_content() {
        use std::fs;
        use std::fs::File;
        use std::io::Write;

        use zip::write::SimpleFileOptions;
        let temp = tempfile::tempdir().expect("archive fixture");
        let archive = temp.path().join("invalid-name.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).expect("archive"));
        zip.start_file("entry.txt", SimpleFileOptions::default())
            .expect("entry");
        zip.write_all(b"content").expect("content");
        zip.finish().expect("finish");
        let mut bytes = fs::read(&archive).expect("archive");
        // Legacy ZIP entries can use CP437 without a UTF-8 flag. Replace one
        // filename byte in both headers while preserving all lengths and CRCs.
        for index in 0..bytes.len().saturating_sub(10) {
            if &bytes[index..index + 9] == b"entry.txt" {
                bytes[index] = 0xff;
            }
        }
        fs::write(&archive, bytes).expect("malformed fixture");
        let output = temp.path().join("out");
        crate::package::extract(&archive, &output).expect("decode legacy filename");
        assert_eq!(
            fs::read(output.join("\u{a0}ntry.txt")).expect("decoded member"),
            b"content"
        );
    }
    #[test]
    fn test_extraction_budget_rejects_oversized_members_before_creating_them() {
        use std::io::Cursor;

        let temp = tempfile::tempdir().expect("output fixture");
        let output = temp.path().join("oversized");
        let mut budget = crate::package::MAX_BYTES;
        let error = crate::package::write_member(&output, &mut Cursor::new(b"x"), budget + 1, &mut budget)
            .expect_err("size exceeds total budget");
        assert_eq!(error.exit_code(), 1);
        assert!(error.to_string().contains("size budget"));
        assert!(!output.exists());
    }
}
