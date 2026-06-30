// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::env;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use os_info;

fn get_lib_path(nixl_root_path: &str, arch: &str) -> String {
    let os_info = os_info::get();
    match os_info.os_type() {
        os_info::Type::Redhat
        | os_info::Type::RedHatEnterprise
        | os_info::Type::CentOS
        | os_info::Type::Fedora => {
            format!("{}/lib64", nixl_root_path)
        }
        os_info::Type::Ubuntu | os_info::Type::Debian => {
            format!("{}/lib/{}-linux-gnu", nixl_root_path, arch)
        }
        os_info::Type::Arch => {
            format!("{}/lib", nixl_root_path)
        }
        _ => {
            // For unknown distributions, try to detect the path dynamically
            let possible_paths = [
                format!("{}/lib64", nixl_root_path),
                format!("{}/lib/{}-linux-gnu", nixl_root_path, arch),
                format!("{}/lib", nixl_root_path),
            ];
            // Print a warning about unknown distribution
            println!(
                "cargo:warning=Unknown Linux distribution: {}. Trying common library paths.",
                os_info
            );
            // Return the first path that exists
            for path in possible_paths.iter() {
                if std::path::Path::new(path).exists() {
                    return path.clone();
                }
            }
            // If no path exists, default to lib64 as it's most common
            format!("{}/lib64", nixl_root_path)
        }
    }
}

fn get_arch() -> String {
    let os_info = os_info::get();
    match os_info.architecture().unwrap_or("x86_64").to_string() {
        arch if arch == "x86_64" => "x86_64".to_string(),
        arch if arch == "aarch64" || arch == "arm64" => "aarch64".to_string(),
        other => panic!("Unsupported architecture: {}", other),
    }
}

fn get_nixl_libs() -> Option<Vec<pkg_config::Library>> {
    // Try to get all libraries, but return None if any fails
    match (
        pkg_config::probe_library("nixl"),
        pkg_config::probe_library("nixl_build"),
        pkg_config::probe_library("nixl_common"),
        pkg_config::probe_library("stream"),
        pkg_config::probe_library("serdes"),
        pkg_config::probe_library("ucx_utils"),
        pkg_config::probe_library("etcd-cpp-api"),
        pkg_config::probe_library("ucx"),
    ) {
        (Ok(nixl), Ok(nixl_build), Ok(nixl_common), Ok(stream), Ok(serdes), Ok(ucx_utils), Ok(etcd), Ok(ucx)) => {
            Some(vec![nixl, nixl_build, nixl_common, stream, serdes, ucx_utils, etcd, ucx])
        }
        _ => None,
    }
}

fn build_nixl(cc_builder: &mut cc::Build, use_pkg_config: bool) -> anyhow::Result<String> {
    let nixl_root_path =
        env::var("NIXL_PREFIX").unwrap_or_else(|_| "/opt/nvidia/nvda_nixl".to_string());

    // Print the NIXL_PREFIX for debugging
    println!("cargo:warning=Using NIXL_PREFIX: {}", nixl_root_path);

    let nixl_include_path = format!("{}/include", nixl_root_path);
    let nixl_include_paths = [
        &nixl_include_path,
        "../../api/cpp",
        "../../infra",
        "../../core",
        "/usr/include",
    ];

    let arch = get_arch();
    let nixl_lib_path = get_lib_path(&nixl_root_path, &arch);

    // Print the library path for debugging
    println!("cargo:warning=Using library path: {}", nixl_lib_path);

    // Collect all candidate library directories: NIXL_PREFIX-derived paths
    // first, then any paths reported by pkg-config.
    let mut lib_search_paths = vec![
        nixl_lib_path.clone(),
        nixl_root_path.clone(),
        format!("{}/lib", nixl_root_path),
        format!("{}/lib64", nixl_root_path),
        format!("{}/lib/{}-linux-gnu", nixl_root_path, arch),
    ];

    // Try to use pkg-config if available, and collect its library paths.
    // Skipped in source-build mode: probing the system would emit link-search
    // metadata for an installed nixl and risk mixing it with the freshly built
    // libraries under NIXL_PREFIX.
    if use_pkg_config {
        if let Some(libs) = get_nixl_libs() {
            println!("cargo:warning=Using pkg-config paths");
            for lib in &libs {
                for path in &lib.link_paths {
                    lib_search_paths.push(path.display().to_string());
                }
            }
        } else {
            println!("cargo:warning=pkg-config not available, using manual library paths");
        }
    } else {
        println!("cargo:warning=source-build mode: using NIXL_PREFIX library paths only");
    }

    // Verify that nixl shared libraries actually exist before proceeding, and
    // remember the directory that actually contained libnixl.so (returned to the
    // caller for rpath/`DEP_NIXL_LIB_DIR`, since get_lib_path's guess can differ
    // from where meson installed on some distros).
    let found_lib_dir = lib_search_paths
        .iter()
        .find(|dir| std::path::Path::new(&format!("{}/libnixl.so", dir)).exists())
        .cloned();
    let found_lib_dir = match found_lib_dir {
        Some(dir) => dir,
        None => {
            return Err(anyhow::anyhow!(
                "libnixl.so not found in any search path {:?}; nixl libraries are not installed",
                lib_search_paths
            ));
        }
    };

    for path in &lib_search_paths {
        println!("cargo:rustc-link-search=native={}", path);
    }

    cc_builder
        .file("wrapper.cpp")
        .includes(nixl_include_paths);

    let etcd_enabled = env::var("HAVE_ETCD").map(|v| v != "0").unwrap_or(false);

    if etcd_enabled {
        cc_builder.define("HAVE_ETCD", "1");
    }

    // Compile the wrapper C++ code
    cc_builder.try_compile("nixl_wrapper")?;

    // Get the output path for bindings
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Generate bindings with minimal configuration
    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg("-std=c++20")
        .clang_arg(format!("-I{}", nixl_include_path))
        .clang_arg("-I../../api/cpp")
        .clang_arg("-I../../infra")
        .clang_arg("-I../../core")
        .clang_arg("-x")
        .clang_arg("c++");

    // Add system include paths if needed
    if let Ok(cpp_include) = env::var("CPLUS_INCLUDE_PATH") {
        for path in cpp_include.split(':') {
            builder = builder.clang_arg(format!("-I{}", path));
        }
    }

    // Link against required libraries
    println!("cargo:rustc-link-lib=stdc++");

    // Add NIXL libraries
    println!("cargo:rustc-link-lib=dylib=nixl");
    println!("cargo:rustc-link-lib=dylib=nixl_build");
    println!("cargo:rustc-link-lib=dylib=nixl_common");

    if etcd_enabled {
        println!("cargo:rustc-link-lib=dylib=etcd-cpp-api");
    }

    // Tell cargo to invalidate the built crate whenever the wrapper changes
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=wrapper.cpp");
    println!("cargo:rerun-if-env-changed=HAVE_ETCD");

    builder
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .map_err(|_| anyhow::anyhow!("Unable to generate bindings"))?
        .write_to_file(out_path.join("bindings.rs"))?;

    Ok(found_lib_dir)
}

fn build_stubs(cc_builder: &mut cc::Build) {
    println!("cargo:warning=Building with stub API - NIXL functions will be resolved at runtime via dlopen");

    cc_builder.file("stubs.cpp");

    cc_builder.compile("nixl_stubs");

    // Link against C++ standard library and libdl (for dlopen/dlsym)
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-link-lib=dylib=dl");

    // Tell cargo to invalidate the built crate whenever the stubs change
    println!("cargo:rerun-if-changed=stubs.cpp");
    println!("cargo:rerun-if-changed=wrapper.h");

    // Get the output path for bindings
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Generate bindings with minimal configuration
    bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg("-std=c++20")
        .clang_arg("-x")
        .clang_arg("c++")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}

fn create_builder() -> cc::Build {
    let mut builder = cc::Build::new();
    builder.cpp(true);
    // Honor an explicit CXX (cc reads it when no compiler is forced) so the
    // wrapper is built with the same toolchain meson used; default to g++.
    if env::var_os("CXX").is_none() {
        builder.compiler("g++");
    }
    builder
        .flag("-std=c++20")
        .flag("-fPIC")
        .flag("-Wno-unused-parameter")
        .flag("-Wno-unused-variable");
    builder
}

fn run_build(use_stub_api: bool) {
    let mut cc_builder = create_builder();

    if !use_stub_api {
        let no_fallback = env::var("NIXL_NO_STUBS_FALLBACK")
            .map(|v| v == "1")
            .unwrap_or(false);

        if let Err(e) = build_nixl(&mut cc_builder, true) {
            if !no_fallback {
                println!(
                    "cargo:warning=NIXL build failed: {}, falling back to stub API",
                    e
                );
                let mut stub_builder = create_builder();
                build_stubs(&mut stub_builder);
            } else {
                panic!("Failed to build NIXL: {}", e);
            }
        }
    } else {
        build_stubs(&mut cc_builder);
    }
}

/// Resolve the nixl C++ source tree to build from, in priority order:
/// 1. `NIXL_SOURCE_DIR` env var (an explicit checkout);
/// 2. the enclosing nixl checkout when building in-tree;
/// 3. sources vendored under the crate (`vendor/nixl`) in a published crate.
///
/// The in-tree checkout is preferred over `vendor/` so that local development
/// always builds the live tree; the published crate has no enclosing checkout
/// and falls through to the vendored sources.
fn resolve_source_dir() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = env::var("NIXL_SOURCE_DIR") {
        if !dir.is_empty() {
            let path = PathBuf::from(dir);
            if is_nixl_source(&path) {
                return Ok(path);
            }
            return Err(anyhow::anyhow!(
                "NIXL_SOURCE_DIR={} is not a nixl source tree (missing meson.build/meson_options.txt)",
                path.display()
            ));
        }
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let manifest_canon = manifest_dir
        .canonicalize()
        .unwrap_or_else(|_| manifest_dir.clone());

    // In-tree build: this crate is exactly <nixl-root>/src/bindings/rust. Match
    // that precise layout rather than walking up to any ancestor with the
    // generic meson files — otherwise a packaged crate verified under
    // `<root>/target/package/...` would wrongly pick the live checkout.
    if let Some(root) = manifest_dir.ancestors().nth(3) {
        let bindings = root.join("src").join("bindings").join("rust");
        let bindings_canon = bindings.canonicalize().unwrap_or(bindings);
        if bindings_canon == manifest_canon && is_nixl_source(root) {
            return Ok(root.to_path_buf());
        }
    }

    // Sources vendored into the published crate by vendor-nixl.sh.
    let vendored = manifest_dir.join("vendor").join("nixl");
    if is_nixl_source(&vendored) {
        return Ok(vendored);
    }

    Err(anyhow::anyhow!(
        "could not locate nixl C++ sources; set NIXL_SOURCE_DIR to a nixl checkout"
    ))
}

/// A directory is a nixl source root if it has both the top-level meson files.
fn is_nixl_source(dir: &std::path::Path) -> bool {
    dir.join("meson.build").is_file() && dir.join("meson_options.txt").is_file()
}

/// Hash the immutable wrap inputs (`subprojects/*.wrap` and everything under
/// `subprojects/packagefiles`) so a wrap revision or patch change invalidates
/// the staged, already-extracted subprojects instead of compiling the old one.
fn hash_wrap_inputs(subprojects: &std::path::Path) -> u64 {
    fn collect(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, out);
                } else {
                    out.push(path);
                }
            }
        }
    }

    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(subprojects) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "wrap") {
                files.push(path);
            }
        }
    }
    collect(&subprojects.join("packagefiles"), &mut files);
    files.sort();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for file in files {
        file.to_string_lossy().hash(&mut hasher);
        if let Ok(bytes) = std::fs::read(&file) {
            bytes.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// A `meson` command scoped to the staged build:
/// - `GIT_CEILING_DIRECTORIES` stops nixl's git-revision detection from walking
///   out of the stage into a consumer repository (the stage has no `.git`).
/// - `DESTDIR` is cleared so `meson install` writes to the real `OUT_DIR` prefix
///   that `build_nixl` then reads, regardless of an inherited environment.
fn meson_cmd(out_dir: &std::path::Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("meson");
    cmd.env("GIT_CEILING_DIRECTORIES", out_dir);
    cmd.env_remove("DESTDIR");
    cmd
}

fn run_command(cmd: &mut std::process::Command, label: &str) -> anyhow::Result<()> {
    println!("cargo:warning=Running {}: {:?}", label, cmd);
    let status = cmd
        .status()
        .map_err(|e| anyhow::anyhow!("failed to spawn `{}`: {}", label, e))?;
    if !status.success() {
        return Err(anyhow::anyhow!("`{}` failed with {}", label, status));
    }
    Ok(())
}

/// Top-level entries meson needs from a nixl source tree. Only these are staged
/// (an in-tree checkout also holds docs/benchmark/test/examples/etc. that the
/// `-Dbuild_tests=false -Dbuild_examples=false` build never reads), bounding the
/// copy and matching what vendor-nixl.sh ships.
const STAGE_TOPLEVEL: &[&str] = &[
    "meson.build",
    "meson_options.txt",
    "nixl.pc.in",
    "src",
    "subprojects",
];

/// Mirror the needed source entries into a writable staging dir under `OUT_DIR`,
/// so meson's wrap downloads and build state never touch the (possibly
/// read-only) source tree. The sync is incremental (size+mtime) to preserve
/// meson's incremental compilation, prunes entries deleted from the source, and
/// skips cargo target dirs (identified by `CACHEDIR.TAG`, regardless of name or
/// a nested `CARGO_TARGET_DIR`), meson build dirs (`meson-info`), and this
/// crate's own `OUT_DIR`. Downloaded subprojects under `<stage>/subprojects`
/// are never pruned.
fn stage_source(
    src: &std::path::Path,
    stage: &std::path::Path,
    out_dir: &std::path::Path,
) -> std::io::Result<()> {
    std::fs::create_dir_all(stage)?;
    let out_dir_abs = out_dir.canonicalize().unwrap_or_else(|_| out_dir.to_path_buf());
    let preserve = stage.join("subprojects");
    for name in STAGE_TOPLEVEL {
        let from = src.join(name);
        let to = stage.join(name);
        match std::fs::metadata(&from) {
            Ok(meta) if meta.is_dir() => sync_dir(&from, &to, &out_dir_abs, &preserve)?,
            Ok(meta) if meta.is_file() => stage_file(&from, &to)?,
            _ => {}
        }
    }
    Ok(())
}

/// Copy a single file into the stage if missing or changed, preserving the
/// source mtime so the next run's size+mtime equality check stays stable.
fn stage_file(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    let src_meta = std::fs::metadata(from)?;
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Replace a stale directory at the destination with the file.
    if to.is_dir() {
        std::fs::remove_dir_all(to)?;
    }
    let src_mtime = src_meta.modified()?;
    let stale = match std::fs::metadata(to) {
        Ok(dst) => dst.len() != src_meta.len() || dst.modified()? != src_mtime,
        Err(_) => true,
    };
    if stale {
        std::fs::copy(from, to)?;
        // fs::copy stamps `to` with "now"; restore the source mtime.
        if let Ok(f) = std::fs::File::options().write(true).open(to) {
            let _ = f.set_modified(src_mtime);
        }
    }
    Ok(())
}

fn is_skipped_dir(path: &std::path::Path, out_dir_abs: &std::path::Path) -> bool {
    if path.join("CACHEDIR.TAG").exists() || path.join("meson-info").is_dir() {
        return true;
    }
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    abs == *out_dir_abs
}

fn sync_dir(
    from_dir: &std::path::Path,
    to_dir: &std::path::Path,
    out_dir_abs: &std::path::Path,
    preserve: &std::path::Path,
) -> std::io::Result<()> {
    std::fs::create_dir_all(to_dir)?;

    // Copy/update entries present in the source.
    for entry in std::fs::read_dir(from_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let ft = entry.file_type()?;
        let from = entry.path();
        let to = to_dir.join(&name);
        if ft.is_dir() {
            if is_skipped_dir(&from, out_dir_abs) {
                continue;
            }
            // Replace a stale file at the destination with the directory.
            if to.is_file() {
                std::fs::remove_file(&to)?;
            }
            sync_dir(&from, &to, out_dir_abs, preserve)?;
        } else if ft.is_file() || ft.is_symlink() {
            // Resolve symlinks to their target; skip dangling/dir symlinks.
            // Re-stages on any size/mtime difference (handles a source swapped
            // for an older tree, not just a newer one).
            if std::fs::metadata(&from).map(|m| m.is_file()).unwrap_or(false) {
                stage_file(&from, &to)?;
            }
        }
    }

    // Prune staged entries no longer in the source. Skip the subprojects subtree
    // so meson's downloaded/extracted dependencies are preserved.
    if to_dir != preserve && !to_dir.starts_with(preserve) {
        for entry in std::fs::read_dir(to_dir)? {
            let entry = entry?;
            if from_dir.join(entry.file_name()).symlink_metadata().is_err() {
                let path = entry.path();
                if entry.file_type()?.is_dir() {
                    let _ = std::fs::remove_dir_all(&path);
                } else {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

    Ok(())
}

/// Environment variables that influence meson's compiler/dependency discovery.
/// Meson captures these only at initial `setup`, so a change must invalidate the
/// cached build; they are tracked for rerun and folded into the fingerprint.
const MESON_ENV_VARS: &[&str] = &[
    "CC",
    "CXX",
    "CFLAGS",
    "CXXFLAGS",
    "CPPFLAGS",
    "LDFLAGS",
    "PKG_CONFIG_PATH",
    "CMAKE_PREFIX_PATH",
    "CUDA_HOME",
];

/// Build the nixl C++ libraries from source with meson into `OUT_DIR`, and
/// return the install prefix (suitable for use as `NIXL_PREFIX`).
fn build_from_source() -> anyhow::Result<PathBuf> {
    let src = resolve_source_dir()?;
    println!("cargo:warning=Building nixl from source: {}", src.display());

    // Rerun when source-build inputs change. Track the C++ tree and meson files
    // (cargo scans directories recursively) so edits trigger a recompile.
    for input in ["src", "meson.build", "meson_options.txt"] {
        let path = src.join(input);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    // Track the immutable wrap/patch inputs, but not the whole `subprojects/`
    // dir: downloads land under the *stage*, so the wrap definitions and
    // packagefiles are the only source-side inputs that affect the build.
    let subprojects = src.join("subprojects");
    if let Ok(entries) = std::fs::read_dir(&subprojects) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "wrap") {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    let packagefiles = subprojects.join("packagefiles");
    if packagefiles.exists() {
        println!("cargo:rerun-if-changed={}", packagefiles.display());
    }
    for var in [
        "NIXL_SOURCE_DIR",
        "NIXL_PLUGINS",
        "NIXL_UCX_PATH",
        "NIXL_CUDA_INC_PATH",
        "NIXL_CUDA_LIB_PATH",
    ] {
        println!("cargo:rerun-if-env-changed={}", var);
    }
    for var in MESON_ENV_VARS {
        println!("cargo:rerun-if-env-changed={}", var);
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let stage = out_dir.join("nixl-src");
    let build_dir = out_dir.join("nixl-build");
    let prefix = out_dir.join("nixl");

    // Effective configuration. These options are always passed explicitly (with
    // their meson defaults when unset) so that a value removed between builds
    // resets the cached option on reconfigure rather than lingering.
    let plugins = env::var("NIXL_PLUGINS").unwrap_or_default();
    let ucx_path = env::var("NIXL_UCX_PATH").unwrap_or_default();
    let cuda_inc = env::var("NIXL_CUDA_INC_PATH").unwrap_or_default();
    let cuda_lib = env::var("NIXL_CUDA_LIB_PATH").unwrap_or_default();

    // Fingerprint the configuration; when it changes, wipe the stage and build
    // dirs so stale options don't linger. (The install prefix is always recreated
    // below, so removed/disabled plugin DSOs never survive a rebuild either.)
    let mut config = format!(
        "src={}\nplugins={}\nucx={}\ncuda_inc={}\ncuda_lib={}\nwraps={:016x}\n",
        src.display(),
        plugins,
        ucx_path,
        cuda_inc,
        cuda_lib,
        hash_wrap_inputs(&subprojects)
    );
    // Fold meson's compiler/dependency discovery environment into the
    // fingerprint so a toolchain/search-path change re-runs setup.
    for var in MESON_ENV_VARS {
        config.push_str(&format!("{}={}\n", var, env::var(var).unwrap_or_default()));
    }
    let fingerprint_file = out_dir.join("nixl-config.fingerprint");
    let config_changed = std::fs::read_to_string(&fingerprint_file).ok().as_deref() != Some(&config);
    if config_changed {
        let _ = std::fs::remove_dir_all(&stage);
        let _ = std::fs::remove_dir_all(&build_dir);
    }

    // Stage the source into OUT_DIR so meson never mutates the (possibly
    // read-only) source tree; meson downloads wrap subprojects under the stage.
    stage_source(&src, &stage, &out_dir)
        .map_err(|e| anyhow::anyhow!("failed to stage source {}: {}", src.display(), e))?;

    let mut setup = meson_cmd(&out_dir);
    setup
        .arg("setup")
        .arg(&build_dir)
        .arg(&stage)
        .arg(format!("--prefix={}", prefix.display()))
        .arg("--buildtype=release")
        .arg("-Dbuild_python=false")
        .arg("-Drust=false")
        .arg("-Dbuild_tests=false")
        .arg("-Dbuild_examples=false")
        .arg(format!("-Denable_plugins={}", plugins))
        .arg(format!("-Ducx_path={}", ucx_path))
        .arg(format!("-Dcudapath_inc={}", cuda_inc))
        .arg(format!("-Dcudapath_lib={}", cuda_lib));

    // meson refuses `setup` on an already-configured build dir, so pass
    // --reconfigure when one exists. (Cargo's rerun-if-* only controls *when*
    // this build script runs; it can't satisfy meson's own setup vs.
    // reconfigure requirement. After a config change the dir was wiped above,
    // so meson-info is absent and a fresh setup runs instead.)
    if build_dir.join("meson-info").is_dir() {
        setup.arg("--reconfigure");
    }

    run_command(&mut setup, "meson setup")?;

    // Record the fingerprint once setup succeeds (i.e. wrap subprojects are
    // downloaded/extracted). A later compile/install failure then leaves the
    // config "unchanged", so a retry preserves the stage and its downloads
    // instead of wiping and re-downloading them (which would break offline).
    std::fs::write(&fingerprint_file, &config)?;

    // Bound ninja's parallelism to Cargo's job limit (NUM_JOBS) so a heavy CUDA
    // build does not oversubscribe the machine when the caller passed --jobs.
    let mut compile = meson_cmd(&out_dir);
    compile.arg("compile").arg("-C").arg(&build_dir);
    if let Ok(jobs) = env::var("NUM_JOBS") {
        compile.arg("-j").arg(jobs);
    }
    run_command(
        &mut compile,
        "meson compile",
    )?;

    // Recreate the prefix so a rebuild never leaves DSOs of removed/disabled
    // plugins behind (meson install does not prune deleted targets).
    let _ = std::fs::remove_dir_all(&prefix);
    run_command(
        meson_cmd(&out_dir).arg("install").arg("-C").arg(&build_dir),
        "meson install",
    )?;

    Ok(prefix)
}

fn run_build_from_source() {
    let prefix = build_from_source()
        .unwrap_or_else(|e| panic!("Failed to build nixl from source: {}", e));

    // Point the existing installed-nixl path at our freshly built prefix and
    // reuse build_nixl for header/link/bindgen wiring.
    env::set_var("NIXL_PREFIX", &prefix);
    let mut cc_builder = create_builder();
    let libdir = build_nixl(&mut cc_builder, false).unwrap_or_else(|e| {
        panic!(
            "Failed to link nixl built from source at {}: {}",
            prefix.display(),
            e
        )
    });

    // Emit an rpath to the directory that actually holds libnixl.so. Note
    // `rustc-link-arg` applies only to nixl-sys' own artifacts (its
    // tests/binaries), not to downstream executables — those must locate the
    // .so at runtime themselves.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{libdir}");

    // Export the install prefix and lib dir as `links` metadata. Because the
    // package sets `links = "nixl"`, direct dependents receive these as
    // DEP_NIXL_ROOT / DEP_NIXL_LIB_DIR and can apply their own rpath so the
    // built shared libraries are found at runtime without LD_LIBRARY_PATH.
    // See README for the downstream build-script snippet.
    println!("cargo:root={}", prefix.display());
    println!("cargo:lib_dir={libdir}");
}

fn main() {
    // `build-from-source` takes precedence: if a consumer opts into building the
    // C++ libs, link them even when feature unification also turns on `stub-api`.
    if cfg!(feature = "build-from-source") {
        run_build_from_source();
    } else {
        run_build(cfg!(feature = "stub-api"));
    }
}
