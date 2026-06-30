// Compiles the OpenVPN3 core + C-ABI shim (native/) into a shared library via
// CMake + vcpkg, links it into the Rust binary, stages the runtime shared
// libraries next to the executable, and (on Windows) embeds a UAC manifest so
// the app elevates to Administrator on launch.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let native = manifest_dir.join("native");

    for f in ["ovpn_shim.cpp", "ovpn_ffi.h", "CMakeLists.txt"] {
        println!("cargo:rerun-if-changed={}", native.join(f).display());
    }
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // On Windows, require Administrator so the tun adapter and split-tunnel
    // routes can be set up — double-clicking the exe then triggers a UAC prompt.
    if target_os == "windows" {
        embed_uac_manifest();
    }

    let vcpkg_root = env::var("VCPKG_ROOT").expect(
        "VCPKG_ROOT must point at a vcpkg checkout with the OpenVPN3 deps installed \
         (asio fmt jsoncpp lz4 openssl tap-windows6 xxhash pkgconf)",
    );
    let vcpkg_root_unix = vcpkg_root.replace('\\', "/");
    let toolchain = format!("{vcpkg_root_unix}/scripts/buildsystems/vcpkg.cmake");
    // Use dynamic-linkage triplets everywhere so the OpenVPN3 shared library and
    // its dependencies are .dll/.so/.dylib we can stage next to the executable.
    let triplet = match target_os.as_str() {
        "windows" => "x64-windows",
        "macos" => "x64-osx",
        _ => "x64-linux-dynamic",
    };

    let mut cfg = cmake::Config::new(&native);
    // Ninja avoids the Visual Studio generator's instance discovery, which fails
    // for non-default VS install locations; the cmake crate still injects the
    // MSVC environment it discovers via vswhere.
    cfg.generator("Ninja")
        .profile("Release")
        .define("CMAKE_TOOLCHAIN_FILE", &toolchain)
        .define("VCPKG_TARGET_TRIPLET", triplet);
    let dst = cfg.build();

    println!("cargo:rustc-link-search=native={}", dst.join("lib").display());
    println!("cargo:rustc-link-lib=dylib=ovpnffi");

    // Let the binary find the staged shared library (and, for local builds, the
    // vcpkg runtime libs) without a global install.
    if target_os == "linux" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{vcpkg_root_unix}/installed/x64-linux-dynamic/lib");
    } else if target_os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{vcpkg_root_unix}/installed/x64-osx/lib");
    }

    stage_runtime_libs(&dst, &target_os);
}

/// Embed an application manifest requesting Administrator elevation (UAC).
fn embed_uac_manifest() {
    use embed_manifest::manifest::ExecutionLevel;
    use embed_manifest::{embed_manifest, new_manifest};

    embed_manifest(
        new_manifest("XMU.SecureLink").requested_execution_level(ExecutionLevel::RequireAdministrator),
    )
    .expect("failed to embed UAC manifest");
}

/// Copy the OpenVPN3 shared library (and any sibling runtime libraries placed by
/// vcpkg's applocal step) next to the cargo output executable.
fn stage_runtime_libs(install_dir: &Path, target_os: &str) {
    let ext: &str = match target_os {
        "windows" => "dll",
        "macos" => "dylib",
        _ => "so",
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out  ->  exe dir is 3 up.
    let Some(exe_dir) = out_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
    else {
        return;
    };

    let mut libs = Vec::new();
    collect_libs(install_dir, ext, &mut libs);
    // The cmake crate keeps the build tree (with applocal-copied deps) alongside.
    collect_libs(&install_dir.join("build"), ext, &mut libs);

    let mut seen = HashSet::new();
    for lib in libs {
        let Some(name) = lib.file_name() else { continue };
        if !seen.insert(name.to_owned()) {
            continue;
        }
        let _ = fs::copy(&lib, exe_dir.join(name));
    }
}

fn collect_libs(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_libs(&path, ext, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
            out.push(path);
        }
    }
}
