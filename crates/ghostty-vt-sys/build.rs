//! Build and link the pinned libghostty-vt 1.0.2 static library.
//!
//! The library is built from the vendored source snapshot in
//! `vendor/ghostty-vt`, not from a moving upstream branch. `zig build`
//! requires `-Demit-lib-vt` for a Git snapshot (release tarballs carry a
//! `LIB_VERSION` file that selects the library build automatically).

use std::env;
use std::path::PathBuf;
use std::process::Command;

const EXPECTED_PIN: &str = "vt-1.0.2 573c4a21e02311ae87844af26b6a7a179f82858e";

fn run(zig: &std::ffi::OsStr, args: &[std::ffi::OsString]) {
    let status = Command::new(zig)
        .args(args)
        .status()
        .unwrap_or_else(|err| panic!("could not run {zig:?}: {err}"));
    assert!(status.success(), "{zig:?} {args:?} failed with {status}");
}

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest_dir.join("../../vendor/ghostty-vt");
    let pin_file = source.join("PIN.txt");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let prefix = out_dir.join("ghostty-vt");
    let cache = out_dir.join("zig-cache");

    println!("cargo:rerun-if-changed={}", pin_file.display());
    println!(
        "cargo:rerun-if-changed={}",
        source.join("build.zig").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        source.join("build.zig.zon").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("chkstk.S").display()
    );
    println!("cargo:rerun-if-env-changed=ZIG");
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_SOURCE");

    let source = env::var_os("GHOSTTY_VT_SOURCE")
        .map(PathBuf::from)
        .unwrap_or(source);
    let pin = std::fs::read_to_string(source.join("PIN.txt")).unwrap_or_default();
    if !pin.contains(EXPECTED_PIN) {
        panic!(
            "vendored libghostty-vt pin mismatch: expected {EXPECTED_PIN} in {:?}",
            source.join("PIN.txt")
        );
    }

    let zig = env::var_os("ZIG").unwrap_or_else(|| "zig".into());
    let status = Command::new(&zig)
        .arg("build")
        .arg("--build-file")
        .arg(source.join("build.zig"))
        .arg("--cache-dir")
        .arg(&cache)
        .arg("--prefix")
        .arg(&prefix)
        .arg("-Demit-lib-vt")
        .arg("-Doptimize=ReleaseFast")
        .arg("-Dcpu=baseline")
        .status()
        .unwrap_or_else(|err| panic!("could not run {zig:?} to build libghostty-vt: {err}"));
    assert!(
        status.success(),
        "zig build -Demit-lib-vt failed with {status}"
    );

    let lib_path = prefix.join("lib");
    // OUT_DIR may be a mix of `/` and `\` on Windows; the MinGW linker treats
    // a trailing path component with backslashes as part of the library name.
    let lib_dir = lib_path.to_string_lossy().replace('\\', "/");
    println!("cargo:rustc-link-search=native={lib_dir}");

    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => {
            // When rustc emits an rlib it resolves `-l static=` using the
            // `lib<name>.a` spelling even on windows-gnu, while Zig installs
            // `ghostty-vt-static.lib`. Provide that alias name for metadata.
            std::fs::copy(
                lib_path.join("ghostty-vt-static.lib"),
                lib_path.join("libghostty-vt-static.a"),
            )
            .expect("alias the Windows static library for rustc");
            // Zig emits `__chkstk` (no COFF decoration); MinGW libgcc exports
            // `___chkstk`. Build a one-instruction shim as a static archive so
            // it is linked before libgcc resolves its external reference.
            let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "x86_64".into());
            let obj = out_dir.join("chkstk.o");
            let compat_dir = out_dir.join("compat");
            std::fs::create_dir_all(&compat_dir).expect("create compat archive dir");
            let archive = compat_dir.join("libpain_chkstk.a");
            use std::ffi::OsString;
            let triple = OsString::from(format!("{arch}-windows-gnu"));
            run(
                zig.as_os_str(),
                &[
                    OsString::from("cc"),
                    OsString::from("-target"),
                    triple,
                    OsString::from("-c"),
                    manifest_dir.join("chkstk.S").into_os_string(),
                    OsString::from("-o"),
                    obj.clone().into_os_string(),
                ],
            );
            run(
                zig.as_os_str(),
                &[
                    OsString::from("ar"),
                    OsString::from("rcs"),
                    archive.clone().into_os_string(),
                    obj.into_os_string(),
                ],
            );
            let compat_dir = compat_dir.to_string_lossy().replace('\\', "/");
            println!("cargo:rustc-link-search=native={compat_dir}");
            println!("cargo:rustc-link-lib=static=ghostty-vt-static");
            println!("cargo:rustc-link-lib=static=pain_chkstk");
            println!("cargo:rustc-link-lib=dylib=ntdll");
            println!("cargo:rustc-link-lib=dylib=kernel32");
        }
        Ok("linux") => {
            println!("cargo:rustc-link-lib=static=ghostty-vt");
            println!("cargo:rustc-link-lib=dylib=pthread");
            println!("cargo:rustc-link-lib=dylib=m");
            println!("cargo:rustc-link-lib=dylib=dl");
        }
        other => panic!("pain M0 only supports windows and linux, got {other:?}"),
    }
}
