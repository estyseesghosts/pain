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

    // The vendored Zig library carries its own stack-probe object. On a
    // windows-gnu host without MSVC, GNU ld can see both Zig's probe and the
    // small compatibility probe from ghostty-vt-sys. Both implement the same
    // ABI contract; allow the duplicate so the link is deterministic.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
    {
        println!("cargo:rustc-link-arg=-Wl,--allow-multiple-definition");
    }

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    copy_bundled_conpty(&manifest_dir, &out_dir);

    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build_msvc_icon();
        return;
    }
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

/// Copy the pinned, MIT-licensed OpenConsole ConPTY pair next to the built
/// executable. The in-box host strips unknown APCs (Kitty graphics), so an
/// adjacent `conpty.dll` is the only way the packaged application can claim
/// graphics support on Windows.
fn copy_bundled_conpty(manifest_dir: &Path, out_dir: &Path) {
    let bundle = manifest_dir.join("windows/conpty/x64");
    // OUT_DIR is target/<profile>/build/<crate>/out; the Windows loader looks
    // beside the executable, which is target/<profile> (or deps for tests).
    let Some(profile_dir) = out_dir.ancestors().nth(3) else {
        return;
    };
    for directory in [profile_dir.to_path_buf(), profile_dir.join("deps")] {
        if let Err(error) = std::fs::create_dir_all(&directory) {
            panic!("could not create {}: {error}", directory.display());
        }
        for name in ["conpty.dll", "OpenConsole.exe"] {
            let source = bundle.join(name);
            println!("cargo:rerun-if-changed={}", source.display());
            let destination = directory.join(name);
            // Copy every build: the loader must never observe a stale host
            // paired with a different conpty.dll version.
            if let Err(error) = std::fs::copy(&source, &destination) {
                panic!(
                    "could not copy bundled {name} to {}: {error}",
                    destination.display()
                );
            }
        }
    }
}
