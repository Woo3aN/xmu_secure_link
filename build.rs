// Compiles the OpenVPN3 core + C-ABI shim (native/) into a shared library via
// CMake + vcpkg, links it into the Rust binary, stages the runtime shared
// libraries (ours + vcpkg's) next to the executable so the artifact is
// self-contained, and (on Windows) embeds a UAC manifest so the app elevates
// to Administrator on launch.

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
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    // On Windows, require Administrator so the tun adapter and split-tunnel
    // routes can be set up — double-clicking the exe then triggers a UAC prompt.
    if target_os == "windows" {
        embed_uac_manifest();
    }

    let vcpkg_root = env::var("VCPKG_ROOT").expect(
        "VCPKG_ROOT must point at a vcpkg checkout with the OpenVPN3 deps installed \
         (asio fmt jsoncpp lz4 openssl tap-windows6 xxhash pkgconf)",
    );
    let vcpkg_root = PathBuf::from(vcpkg_root.replace('\\', "/"));
    let toolchain = vcpkg_root.join("scripts/buildsystems/vcpkg.cmake");

    // Pick the vcpkg triplet from arch + OS. Use dynamic-linkage everywhere so
    // OpenVPN3 and its deps are .dll/.so/.dylib we can stage beside the binary.
    let vc_arch = match target_arch.as_str() {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    let triplet = match target_os.as_str() {
        "windows" => format!("{vc_arch}-windows"),
        "macos" => format!("{vc_arch}-osx"),
        _ => format!("{vc_arch}-linux-dynamic"),
    };

    let mut cfg = cmake::Config::new(&native);
    // Ninja avoids the Visual Studio generator's instance discovery, which fails
    // for non-default VS install locations; the cmake crate still injects the
    // MSVC environment it discovers via vswhere.
    cfg.generator("Ninja")
        .profile("Release")
        .define("CMAKE_TOOLCHAIN_FILE", toolchain.to_string_lossy().as_ref())
        .define("VCPKG_TARGET_TRIPLET", &triplet);
    let dst = cfg.build();

    println!(
        "cargo:rustc-link-search=native={}",
        dst.join("lib").display()
    );
    println!("cargo:rustc-link-lib=dylib=ovpnffi");

    // arm64 Linux 特判：补上 libovpnffi.so 依赖但没有传递出来的 jsoncpp/C++ runtime。
    if target_os == "linux" && target_arch == "aarch64" {
        let vcpkg_lib_dir = vcpkg_root.join("installed").join(&triplet).join("lib");

        println!("cargo:rustc-link-search=native={}", vcpkg_lib_dir.display());

        // jsoncpp provides Json::Value / Json::StreamWriterBuilder symbols.
        println!("cargo:rustc-link-arg=-Wl,--no-as-needed");
        println!("cargo:rustc-link-lib=dylib=jsoncpp");
        println!("cargo:rustc-link-lib=dylib=stdc++");
        println!("cargo:rustc-link-arg=-Wl,--as-needed");
    }

    // Let the binary find the staged shared libraries via a relative rpath.
    match target_os.as_str() {
        "linux" => println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN"),
        "macos" => println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path"),
        _ => {}
    }

    stage_runtime_libs(&dst, &vcpkg_root, &triplet, &target_os);
}

/// Embed an application manifest requesting Administrator elevation (UAC).
fn embed_uac_manifest() {
    use embed_manifest::manifest::ExecutionLevel;
    use embed_manifest::{embed_manifest, new_manifest};

    embed_manifest(
        new_manifest("XMU.SecureLink")
            .requested_execution_level(ExecutionLevel::RequireAdministrator),
    )
    .expect("failed to embed UAC manifest");
}

/// Copy the OpenVPN3 shared library plus the vcpkg runtime shared libraries it
/// depends on next to the cargo output executable, so the artifact runs without
/// a global install.
fn stage_runtime_libs(install_dir: &Path, vcpkg_root: &Path, triplet: &str, target_os: &str) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out  ->  exe dir is 3 up.
    let Some(exe_dir) = out_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
    else {
        return;
    };

    let is_shared_lib: fn(&str) -> bool = match target_os {
        "windows" => |n| n.ends_with(".dll"),
        "macos" => |n| n.ends_with(".dylib"),
        // Linux shared libs are versioned (libssl.so.3), so match the infix.
        _ => |n| n.contains(".so"),
    };

    let mut libs = Vec::new();
    // Our compiled shared lib. On Windows vcpkg's applocal step has already
    // placed the exact dependency DLLs in the build tree, so collecting there is
    // precise.
    collect_libs(install_dir, &is_shared_lib, &mut libs);
    collect_libs(&install_dir.join("build"), &is_shared_lib, &mut libs);
    // Linux/macOS have no applocal step, so pull the vcpkg runtime libraries for
    // this triplet (release tree only) to make the artifact self-contained.
    if target_os != "windows" {
        let vc_installed = vcpkg_root.join("installed").join(triplet);
        collect_libs(&vc_installed.join("bin"), &is_shared_lib, &mut libs);
        collect_libs(&vc_installed.join("lib"), &is_shared_lib, &mut libs);
    }

    let mut seen = HashSet::new();
    for lib in libs {
        let Some(name) = lib.file_name() else {
            continue;
        };
        if !seen.insert(name.to_owned()) {
            continue;
        }
        let _ = fs::copy(&lib, exe_dir.join(name));
    }
}

fn collect_libs(dir: &Path, is_shared_lib: &fn(&str) -> bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_libs(&path, is_shared_lib, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_shared_lib)
        {
            out.push(path);
        }
    }
}
