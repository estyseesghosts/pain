//! Embeds the pain icon into the Windows executable.
//!
//! GNU hosts use `windres` with forward-slash paths because the local MSYS2
//! `windres` treats backslashes as escapes. MSVC hosts use the Windows SDK's
//! `rc.exe`, found through `embed-resource`, as Chaus does. GPUI embeds its
//! own Common-Controls manifest separately on both toolchains.

use std::env;
use std::ffi::OsStr;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

// The pure formatter lives in src/version.rs so the CLI tests can exercise the
// same shape the build script emits. `cfg(test)` is false in a build script.
mod version {
    include!("src/version.rs");
}

fn git_output(args: &[&OsStr]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    // A source archive has no .git; keep a stable, obviously synthetic value
    // rather than failing the build or inventing a commit.
    let commit_date = git_output(&[
        OsStr::new("show"),
        OsStr::new("-s"),
        OsStr::new("--format=%cs"),
        OsStr::new("HEAD"),
    ]);
    let short_hash = git_output(&[
        OsStr::new("rev-parse"),
        OsStr::new("--short=7"),
        OsStr::new("HEAD"),
    ]);
    let release_version = match (commit_date, short_hash) {
        (Some(date), Some(hash)) if !date.is_empty() && !hash.is_empty() => {
            version::release_version(&date, &hash)
        }
        _ => String::from("v00-00-unknown"),
    };
    println!("cargo:rustc-env=PAIN_RELEASE_VERSION={release_version}");
    // A commit or tag changes the version without changing any source file.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
    println!("cargo:rerun-if-changed=.git/packed-refs");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build_msvc_icon();
        return;
    }

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let resources = manifest_dir.join("resources/windows");
    let resource_file = resources.join("pain.rc");
    let icon_file = resources.join("pain.ico");
    let object_file = out_dir.join("pain-icon.o");

    println!("cargo:rerun-if-changed={}", resource_file.display());
    println!("cargo:rerun-if-changed={}", icon_file.display());

    let input = forward_slashes(&resource_file);
    let include_dir = forward_slashes(&resources);
    let output = forward_slashes(&object_file);
    let result = Command::new("windres")
        .args([
            "--input",
            &input,
            "--output",
            &output,
            "--include-dir",
            &include_dir,
        ])
        .output()
        .expect("windres is required to embed the Windows icon");

    if !result.status.success() {
        panic!(
            "windres could not compile {}:\n{}",
            resource_file.display(),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    println!("cargo:rustc-link-arg-bins={output}");
}

fn build_msvc_icon() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let resources = manifest_dir.join("resources/windows");
    let resource_file = resources.join("pain.rc");
    let icon_file = resources.join("pain.ico");
    let object_file = out_dir.join("pain-icon.res");

    println!("cargo:rerun-if-changed={}", resource_file.display());
    println!("cargo:rerun-if-changed={}", icon_file.display());

    // The Windows SDK bin directory is not on PATH in a plain CI shell.
    let rc =
        embed_resource::find_windows_sdk_tool("rc.exe").unwrap_or_else(|| PathBuf::from("rc.exe"));
    let include = forward_slashes(&resources);
    let object = forward_slashes(&object_file);
    let input = forward_slashes(&resource_file);
    let result = Command::new(rc)
        .args(["/nologo", "/i", &include, "/fo", &object, &input])
        .output()
        .expect("rc.exe is required to embed the Windows icon");

    if !result.status.success() {
        panic!(
            "rc.exe could not compile {}:\n{}",
            resource_file.display(),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    println!("cargo:rustc-link-arg-bins={object}");
}

fn forward_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
