use super::native_binary::{launcher_temp_path, node_executable, node_launcher_script};
#[cfg(unix)]
use super::{InstallPnpmResult, reuse_global_engine};
use super::{
    PNPM_EXE_PACKAGE_NAME, PNPM_PACKAGE_NAME, assert_release_is_installable,
    exe_platform_pkg_dir_name, exe_platform_pkg_dir_name_next, link_exe_platform_binary,
    package_dir, pnpm_package_to_install, reuse_cached_engine, run_install,
};
use pnpm_config::Config;
use pnpm_graph_hasher::{host_arch, host_libc, host_platform};
use pnpm_reporter::SilentReporter;
use pnpm_store_dir::StoreDir;
use pnpm_testing_utils::registry::TestRegistry;
use std::fs;

/// The engine install must stay anchored to its install dir even when an
/// ancestor carries a `pnpm-workspace.yaml` — the global packages dir
/// legitimately holds one of global settings once a global `allowBuilds`
/// decision has been persisted. Left unanchored, the install pipeline
/// walked up, adopted that file as the workspace root, and the wrapper
/// package never landed in `node_modules/` of the install dir
/// (pnpm/pnpm#13697). Driven with a stand-in package from the mock
/// registry: `run_install` lays out whatever package it is given, so the
/// anchoring is observable without a real pnpm release.
#[tokio::test]
async fn run_install_ignores_an_ambient_workspace_manifest_above_the_install_dir() {
    let registry = TestRegistry::start();
    let temp = tempfile::tempdir().expect("tempdir");
    let global_pkg_dir = temp.path().join("global").join("v11");
    fs::create_dir_all(&global_pkg_dir).expect("create global packages dir");
    fs::write(global_pkg_dir.join("pnpm-workspace.yaml"), "allowBuilds:\n  esbuild: true\n")
        .expect("write ambient workspace manifest");
    // The leftovers an unanchored install strands in the global packages
    // dir: a stray `node_modules` and a lockfile at the adopted workspace
    // root. A later self-update must succeed with them in place (each
    // update installs into a fresh slot) and must not touch that lockfile
    // — it is the env lockfile's home.
    fs::create_dir_all(global_pkg_dir.join("node_modules").join(".pnpm"))
        .expect("create leftover node_modules");
    let leftover_lockfile = "lockfileVersion: '9.0'\n";
    fs::write(global_pkg_dir.join("pnpm-lock.yaml"), leftover_lockfile)
        .expect("write leftover lockfile");
    let install_dir = global_pkg_dir.join("engine-slot");
    fs::create_dir_all(&install_dir).expect("create install dir");

    let mut cfg = Config {
        store_dir: StoreDir::new(temp.path().join("store")),
        cache_dir: temp.path().join("cache"),
        ..Config::default()
    };
    cfg.package_manager_bootstrap.registry = registry.url().to_string();
    let config = Config::leak(cfg);

    run_install::<SilentReporter>(
        config,
        &install_dir,
        "@pnpm.e2e/hello-world-js-bin",
        "1.0.0",
        None,
        None,
    )
    .await
    .expect("install the stand-in engine package");

    let manifest = package_dir(&install_dir, "@pnpm.e2e/hello-world-js-bin").join("package.json");
    assert!(
        manifest.exists(),
        "the installed package must land in the install dir, not in an ambient workspace root",
    );
    let ambient_lockfile = fs::read_to_string(global_pkg_dir.join("pnpm-lock.yaml"))
        .expect("read back the global-dir lockfile");
    assert_eq!(
        ambient_lockfile, leftover_lockfile,
        "the install must write its lockfile into the install dir, not over the global dir's",
    );
}

#[tokio::test]
async fn run_install_persists_minimum_release_age_excludes_to_target_workspace() {
    let (temp, workspace_yaml) = engine_install_with_immature_release(true).await;

    let install_manifest = temp.path().join("engine-slot/pnpm-workspace.yaml");
    assert!(!install_manifest.exists());
    let manifest = fs::read_to_string(&workspace_yaml).expect("read workspace yaml");
    assert!(manifest.contains("minimumReleaseAgeExclude:"), "{manifest}");
    assert!(manifest.contains("@pnpm.e2e/hello-world-js-bin@1.0.0"), "{manifest}");
}

#[tokio::test]
async fn run_install_leaves_the_caller_workspace_alone_without_a_target() {
    let (temp, workspace_yaml) = engine_install_with_immature_release(false).await;

    let manifest = fs::read_to_string(&workspace_yaml).expect("read workspace yaml");
    assert_eq!(manifest, "packages:\n  - packages/*\n");
    let install_manifest = fs::read_to_string(temp.path().join("engine-slot/pnpm-workspace.yaml"))
        .expect("read the install dir's workspace yaml");
    assert!(install_manifest.contains("@pnpm.e2e/hello-world-js-bin@1.0.0"), "{install_manifest}");
}

/// Install an immature engine package, with `minimumReleaseAgeStrict` off,
/// on behalf of a workspace that is the install's `target_workspace_dir`
/// when `targeted`. Returns the temp root and that workspace's manifest.
async fn engine_install_with_immature_release(
    targeted: bool,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let registry = TestRegistry::start();
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace_dir = temp.path().join("workspace");
    fs::create_dir_all(&workspace_dir).expect("create workspace dir");
    let workspace_yaml = workspace_dir.join("pnpm-workspace.yaml");
    fs::write(&workspace_yaml, "packages:\n  - packages/*\n").expect("write workspace manifest");

    let install_dir = temp.path().join("engine-slot");
    fs::create_dir_all(&install_dir).expect("create install dir");

    let mut cfg = Config {
        store_dir: StoreDir::new(temp.path().join("store")),
        cache_dir: temp.path().join("cache"),
        workspace_dir: Some(workspace_dir.clone()),
        target_workspace_dir: targeted.then(|| workspace_dir.clone()),
        minimum_release_age: Some(60 * 24 * 365 * 100),
        minimum_release_age_strict: Some(false),
        ..Config::default()
    };
    cfg.package_manager_bootstrap.registry = registry.url().to_string();
    let config = Config::leak(cfg);

    run_install::<SilentReporter>(
        config,
        &install_dir,
        "@pnpm.e2e/hello-world-js-bin",
        "1.0.0",
        None,
        None,
    )
    .await
    .expect("install engine package");
    drop(registry);
    (temp, workspace_yaml)
}

#[test]
fn legacy_platform_dir_names() {
    assert_eq!(exe_platform_pkg_dir_name("darwin", "arm64", "unknown"), "macos-arm64");
    assert_eq!(exe_platform_pkg_dir_name("darwin", "x64", "unknown"), "macos-x64");
    assert_eq!(exe_platform_pkg_dir_name("win32", "x64", "unknown"), "win-x64");
    assert_eq!(exe_platform_pkg_dir_name("win32", "ia32", "unknown"), "win-x86");
    assert_eq!(exe_platform_pkg_dir_name("linux", "x64", "glibc"), "linux-x64");
    assert_eq!(exe_platform_pkg_dir_name("linux", "x64", "musl"), "linuxstatic-x64");
    assert_eq!(exe_platform_pkg_dir_name("linux", "arm64", "musl"), "linuxstatic-arm64");
}

#[test]
fn next_platform_dir_names() {
    assert_eq!(exe_platform_pkg_dir_name_next("darwin", "arm64", "unknown"), "exe.darwin-arm64");
    assert_eq!(exe_platform_pkg_dir_name_next("win32", "ia32", "unknown"), "exe.win32-x86");
    assert_eq!(exe_platform_pkg_dir_name_next("linux", "x64", "glibc"), "exe.linux-x64");
    assert_eq!(exe_platform_pkg_dir_name_next("linux", "x64", "musl"), "exe.linux-x64-musl");
    assert_eq!(exe_platform_pkg_dir_name_next("linux", "arm64", "musl"), "exe.linux-arm64-musl");
}

#[test]
fn target_package_name_matches_pnpm_engine_layout() {
    assert_eq!(pnpm_package_to_install("12.0.0-alpha.1").name, PNPM_PACKAGE_NAME);
    assert_eq!(pnpm_package_to_install("12.0.0").name, PNPM_PACKAGE_NAME);
    assert_eq!(pnpm_package_to_install("11.10.0").name, PNPM_EXE_PACKAGE_NAME);
    assert_eq!(pnpm_package_to_install("10.34.4").name, PNPM_EXE_PACKAGE_NAME);
    assert_eq!(pnpm_package_to_install("6.17.1").name, PNPM_EXE_PACKAGE_NAME);
    assert_eq!(pnpm_package_to_install("6.16.0").name, PNPM_PACKAGE_NAME);
    assert_eq!(pnpm_package_to_install("5.18.10").name, PNPM_PACKAGE_NAME);
    assert_eq!(pnpm_package_to_install("not-semver").name, PNPM_EXE_PACKAGE_NAME);
}

#[test]
fn native_binary_linking_matches_pnpm_engine_layout() {
    assert!(pnpm_package_to_install("12.0.0-alpha.1").links_native_binary);
    assert!(pnpm_package_to_install("11.10.0").links_native_binary);
    assert!(pnpm_package_to_install("6.17.1").links_native_binary);
    assert!(!pnpm_package_to_install("6.16.0").links_native_binary);
    assert!(!pnpm_package_to_install("5.18.10").links_native_binary);
    assert!(pnpm_package_to_install("not-semver").links_native_binary);
}

/// Lay out a fake engine install: the `pnpm` wrapper and, under
/// `@pnpm/<host-platform-dir>`, the native binary the wrapper's preinstall
/// would normally link.
fn fake_engine_install(install_dir: &std::path::Path, with_native_binary: bool) {
    fake_engine_install_for(install_dir, PNPM_PACKAGE_NAME, with_native_binary);
}

fn fake_engine_install_for(
    install_dir: &std::path::Path,
    wrapper_pkg_name: &str,
    with_native_binary: bool,
) {
    let node_modules = install_dir.join("node_modules");
    fs::create_dir_all(package_dir(install_dir, wrapper_pkg_name)).expect("create wrapper dir");
    if with_native_binary {
        let platform_dir =
            exe_platform_pkg_dir_name_next(host_platform(), host_arch(), host_libc());
        let src_dir = node_modules.join("@pnpm").join(platform_dir);
        fs::create_dir_all(&src_dir).expect("create platform dir");
        fs::write(src_dir.join("pnpm"), b"#!/bin/sh\necho pnpm\n").expect("write native binary");
    }
}

#[cfg(unix)]
#[test]
fn links_the_host_platform_binary_into_the_wrapper() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    fake_engine_install(temp.path(), true);

    link_exe_platform_binary(temp.path(), "pnpm").expect("linking should succeed");

    let dest = temp
        .path()
        .join("node_modules")
        .join("pnpm")
        .join("pnpm");
    assert!(dest.exists(), "the native binary is linked into the wrapper");
    assert_eq!(fs::read(&dest).expect("read linked binary"), b"#!/bin/sh\necho pnpm\n");
    let mode = fs::metadata(&dest)
        .expect("stat linked binary")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755, "the linked binary is executable");
}

#[cfg(unix)]
#[test]
fn links_the_host_platform_binary_into_scoped_exe_wrapper() {
    let temp = tempfile::tempdir().expect("tempdir");
    fake_engine_install_for(temp.path(), PNPM_EXE_PACKAGE_NAME, true);

    link_exe_platform_binary(temp.path(), PNPM_EXE_PACKAGE_NAME).expect("linking should succeed");

    let dest = package_dir(temp.path(), PNPM_EXE_PACKAGE_NAME).join("pnpm");
    assert!(dest.exists(), "the native binary is linked into the scoped wrapper");
    assert_eq!(fs::read(&dest).expect("read linked binary"), b"#!/bin/sh\necho pnpm\n");
}

/// Lay out the wrapper slot of a fake global-virtual-store engine install
/// (`links/<scope-or-@>/<name>/<version>/<hash>`) and return the slot
/// directory. The platform package is left to each test: the real installer
/// materializes it as a symlink to a sibling slot, which is exactly the
/// resolution under test.
#[cfg(unix)]
fn fake_gvs_wrapper_slot(
    links_dir: &std::path::Path,
    wrapper_pkg_name: &str,
) -> std::path::PathBuf {
    let slot = match wrapper_pkg_name.split_once('/') {
        Some((scope, name)) => links_dir.join(scope).join(name),
        None => links_dir.join("@").join(wrapper_pkg_name),
    }
    .join("12.0.0-alpha.7")
    .join("cafe0123");
    fs::create_dir_all(package_dir(&slot, wrapper_pkg_name)).expect("create wrapper dir");
    fs::create_dir_all(slot.join("node_modules").join("@pnpm")).expect("create scope dir");
    slot
}

/// Materialize the native platform package in its own sibling slot under
/// `links` and return the package directory the wrapper's platform symlink
/// should point at.
#[cfg(unix)]
fn fake_gvs_native_slot(links_dir: &std::path::Path) -> std::path::PathBuf {
    let platform_dir = exe_platform_pkg_dir_name_next(host_platform(), host_arch(), host_libc());
    let native_pkg_dir = links_dir
        .join("@pnpm")
        .join(&platform_dir)
        .join("12.0.0-alpha.7")
        .join("beef4567")
        .join("node_modules")
        .join("@pnpm")
        .join(&platform_dir);
    fs::create_dir_all(&native_pkg_dir).expect("create native package dir");
    fs::write(native_pkg_dir.join("pnpm"), b"#!/bin/sh\necho pnpm\n").expect("write native binary");
    native_pkg_dir
}

/// `packageManager` delegation installs the engine into the global virtual
/// store, where the unscoped `pnpm` wrapper sits under the `@` placeholder
/// scope and its platform package legitimately resolves into a sibling slot
/// — the trust root must widen to `links` instead of the wrapper's own slot.
#[cfg(unix)]
#[test]
fn links_native_binary_from_a_sibling_global_virtual_store_slot() {
    let temp = tempfile::tempdir().expect("tempdir");
    let links_dir = temp.path().join("links");
    let slot = fake_gvs_wrapper_slot(&links_dir, "pnpm");
    let native_pkg_dir = fake_gvs_native_slot(&links_dir);
    let platform_dir = exe_platform_pkg_dir_name_next(host_platform(), host_arch(), host_libc());
    std::os::unix::fs::symlink(
        &native_pkg_dir,
        slot.join("node_modules")
            .join("@pnpm")
            .join(platform_dir),
    )
    .expect("symlink platform package to the sibling slot");

    link_exe_platform_binary(&slot, "pnpm").expect("linking should succeed");

    let dest = package_dir(&slot, "pnpm").join("pnpm");
    assert!(dest.exists(), "the native binary is linked into the wrapper");
    assert_eq!(fs::read(&dest).expect("read linked binary"), b"#!/bin/sh\necho pnpm\n");
}

#[cfg(unix)]
#[test]
fn links_native_binary_from_a_sibling_slot_into_the_scoped_wrapper() {
    let temp = tempfile::tempdir().expect("tempdir");
    let links_dir = temp.path().join("links");
    let slot = fake_gvs_wrapper_slot(&links_dir, PNPM_EXE_PACKAGE_NAME);
    let native_pkg_dir = fake_gvs_native_slot(&links_dir);
    let platform_dir = exe_platform_pkg_dir_name_next(host_platform(), host_arch(), host_libc());
    std::os::unix::fs::symlink(
        &native_pkg_dir,
        slot.join("node_modules")
            .join("@pnpm")
            .join(platform_dir),
    )
    .expect("symlink platform package to the sibling slot");

    link_exe_platform_binary(&slot, PNPM_EXE_PACKAGE_NAME).expect("linking should succeed");

    assert!(package_dir(&slot, PNPM_EXE_PACKAGE_NAME).join("pnpm").exists());
}

/// A platform-package symlink that leaves `links` entirely must still be
/// rejected even when the wrapper sits in a global-virtual-store slot.
#[cfg(unix)]
#[test]
fn rejects_native_binary_that_escapes_the_global_virtual_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let links_dir = temp.path().join("links");
    let slot = fake_gvs_wrapper_slot(&links_dir, "pnpm");
    let platform_dir = exe_platform_pkg_dir_name_next(host_platform(), host_arch(), host_libc());
    let outside_pkg_dir = outside.path().join(&platform_dir);
    fs::create_dir_all(&outside_pkg_dir).expect("create outside package dir");
    fs::write(outside_pkg_dir.join("pnpm"), b"outside").expect("write outside binary");
    std::os::unix::fs::symlink(
        &outside_pkg_dir,
        slot.join("node_modules")
            .join("@pnpm")
            .join(platform_dir),
    )
    .expect("symlink platform package outside the store");

    let err = link_exe_platform_binary(&slot, "pnpm").expect_err("escaped native source rejected");
    assert!(err.to_string().contains("resolves outside"), "unexpected error: {err:?}");
    assert!(!package_dir(&slot, "pnpm").join("pnpm").exists());
}

#[cfg(unix)]
#[test]
fn rejects_wrapper_symlink_that_escapes_the_install_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let outside_wrapper = outside.path().join("exe");
    fs::create_dir_all(&outside_wrapper).expect("create outside wrapper");
    fs::write(outside_wrapper.join("pnpm"), b"outside").expect("write outside placeholder");

    fs::create_dir_all(
        temp.path()
            .join("node_modules")
            .join("@pnpm"),
    )
    .expect("create scope dir");
    std::os::unix::fs::symlink(&outside_wrapper, package_dir(temp.path(), PNPM_EXE_PACKAGE_NAME))
        .expect("symlink wrapper outside install dir");

    let err = link_exe_platform_binary(temp.path(), PNPM_EXE_PACKAGE_NAME)
        .expect_err("escaped wrapper must be rejected");
    assert!(err.to_string().contains("resolves outside"), "unexpected error: {err:?}");
    assert_eq!(fs::read(outside_wrapper.join("pnpm")).expect("read outside file"), b"outside");
}

#[cfg(unix)]
#[test]
fn rejects_native_binary_symlink_that_escapes_the_install_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let outside_binary = outside.path().join("pnpm");
    fs::write(&outside_binary, b"outside").expect("write outside binary");
    fake_engine_install(temp.path(), false);

    let platform_dir = exe_platform_pkg_dir_name_next(host_platform(), host_arch(), host_libc());
    let src_dir = temp
        .path()
        .join("node_modules")
        .join("@pnpm")
        .join(platform_dir);
    fs::create_dir_all(&src_dir).expect("create platform dir");
    std::os::unix::fs::symlink(&outside_binary, src_dir.join("pnpm"))
        .expect("symlink native binary outside install dir");

    let err =
        link_exe_platform_binary(temp.path(), "pnpm").expect_err("escaped native source rejected");
    assert!(err.to_string().contains("is a symlink"), "unexpected error: {err:?}");
    assert!(!package_dir(temp.path(), "pnpm").join("pnpm").exists());
    assert_eq!(fs::read(outside_binary).expect("read outside file"), b"outside");
}

#[cfg(unix)]
#[test]
fn rejects_native_binary_scope_symlink_that_escapes_the_install_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    fake_engine_install(temp.path(), false);

    let platform_dir = exe_platform_pkg_dir_name_next(host_platform(), host_arch(), host_libc());
    let outside_scope = outside.path().join("@pnpm");
    let outside_platform_dir = outside_scope.join(platform_dir);
    fs::create_dir_all(&outside_platform_dir).expect("create outside platform dir");
    fs::write(outside_platform_dir.join("pnpm"), b"outside").expect("write outside binary");
    std::os::unix::fs::symlink(
        &outside_scope,
        temp.path()
            .join("node_modules")
            .join("@pnpm"),
    )
    .expect("symlink native scope outside install dir");

    let err =
        link_exe_platform_binary(temp.path(), "pnpm").expect_err("escaped native source rejected");
    assert!(err.to_string().contains("resolves outside"), "unexpected error: {err:?}");
    assert!(!package_dir(temp.path(), "pnpm").join("pnpm").exists());
}

#[test]
fn link_errors_when_the_native_binary_is_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    // Wrapper present, but no `@pnpm/<platform>` native binary — linking must
    // fail loudly rather than leave a broken "successful" self-update.
    fake_engine_install(temp.path(), false);

    assert!(link_exe_platform_binary(temp.path(), "pnpm").is_err());
}

/// Write a wrapper `package.json` recording `version` so
/// [`super::installed_version`] reads it back.
fn write_wrapper_version(install_dir: &std::path::Path, wrapper_pkg_name: &str, version: &str) {
    let manifest = format!(r#"{{"name":"{wrapper_pkg_name}","version":"{version}"}}"#);
    fs::write(package_dir(install_dir, wrapper_pkg_name).join("package.json"), manifest)
        .expect("write wrapper package.json");
}

#[cfg(unix)]
#[test]
fn reuse_cached_engine_accepts_a_healthy_slot() {
    let temp = tempfile::tempdir().expect("tempdir");
    fake_engine_install_for(temp.path(), PNPM_EXE_PACKAGE_NAME, true);
    write_wrapper_version(temp.path(), PNPM_EXE_PACKAGE_NAME, "11.10.0");

    assert!(reuse_cached_engine(temp.path(), pnpm_package_to_install("11.10.0"), "11.10.0"));
    // The relink repaired the slot in place: the native binary is now linked.
    assert!(package_dir(temp.path(), PNPM_EXE_PACKAGE_NAME).join("pnpm").exists());
}

/// `pnpm_package_to_install` resolves v12 to `pnpm`, but the standalone
/// install script installs the engine as `@pnpm/exe` (pnpm/pnpm#14823).
#[cfg(unix)]
#[test]
fn reuse_global_engine_accepts_a_v12_engine_installed_as_pnpm_exe() {
    let global_dir = tempfile::tempdir().expect("tempdir");
    let install_dir = seed_global_group(global_dir.path(), PNPM_EXE_PACKAGE_NAME, "12.3.4", true);

    let reused = reuse_target_engine(global_dir.path(), "12.3.4").expect("the group is reused");

    assert!(reused.already_existed);
    assert_eq!(reused.package_name, PNPM_EXE_PACKAGE_NAME);
    assert_eq!(reused.install_dir, fs::canonicalize(&install_dir).expect("canonicalize"));
}

/// Every seeded group records the target version, so the relink is the only
/// thing separating a reusable engine from a dead one.
#[cfg(unix)]
#[test]
fn reuse_global_engine_skips_a_group_it_cannot_relink() {
    let global_dir = tempfile::tempdir().expect("tempdir");
    seed_global_group(global_dir.path(), "cowsay", "12.3.4", false);
    seed_global_group(global_dir.path(), PNPM_PACKAGE_NAME, "12.3.4", false);

    assert!(
        reuse_target_engine(global_dir.path(), "12.3.4").is_none(),
        "a wrapper with no platform binary is not a reusable engine",
    );

    let install_dir = seed_global_group(global_dir.path(), PNPM_EXE_PACKAGE_NAME, "12.3.4", true);
    let reused = reuse_target_engine(global_dir.path(), "12.3.4").expect("the group is reused");

    assert_eq!(reused.package_name, PNPM_EXE_PACKAGE_NAME);
    assert_eq!(reused.install_dir, fs::canonicalize(&install_dir).expect("canonicalize"));
}

#[cfg(unix)]
fn reuse_target_engine(global_dir: &std::path::Path, version: &str) -> Option<InstallPnpmResult> {
    reuse_global_engine(global_dir, pnpm_package_to_install(version), version)
        .expect("scan the global packages dir")
}

/// `scan_global_packages` enumerates the hash symlinks, not the install dirs,
/// so a seeded group is only visible to it once it is linked.
#[cfg(unix)]
fn seed_global_group(
    global_dir: &std::path::Path,
    wrapper_pkg_name: &str,
    version: &str,
    with_native_binary: bool,
) -> std::path::PathBuf {
    let slot = format!("{}-{version}", wrapper_pkg_name.replace(['@', '/'], "-"));
    let install_dir = global_dir.join(format!("engine-{slot}"));
    fake_engine_install_for(&install_dir, wrapper_pkg_name, with_native_binary);
    write_wrapper_version(&install_dir, wrapper_pkg_name, version);
    let manifest = format!(r#"{{"dependencies":{{"{wrapper_pkg_name}":"{version}"}}}}"#);
    fs::write(install_dir.join("package.json"), manifest).expect("write the group manifest");
    pnpm_fs::force_symlink_dir(&install_dir, &global_dir.join(format!("hash-{slot}")))
        .expect("link the group");
    install_dir
}

#[test]
fn reuse_cached_engine_rejects_a_version_mismatch() {
    let temp = tempfile::tempdir().expect("tempdir");
    fake_engine_install_for(temp.path(), PNPM_EXE_PACKAGE_NAME, true);
    write_wrapper_version(temp.path(), PNPM_EXE_PACKAGE_NAME, "11.9.0");

    assert!(!reuse_cached_engine(temp.path(), pnpm_package_to_install("11.10.0"), "11.10.0"));
}

#[test]
fn assert_release_is_installable_refuses_the_broken_releases() {
    for version in ["11.12.0", "11.13.0"] {
        let err = assert_release_is_installable(version).unwrap_err();
        assert!(err.to_string().contains("broken release"), "{err}");
    }
}

#[test]
fn assert_release_is_installable_allows_every_other_release() {
    for version in ["11.11.0", "11.13.1", "12.0.0"] {
        assert_release_is_installable(version).unwrap();
    }
}

/// A slot left by an older layout whose wrapper symlink escapes the slot
/// (e.g. into a shared global virtual store) must not be reused — the
/// caller falls through to a fresh install instead of aborting the whole
/// self-update on the wrapper-containment guard.
#[cfg(unix)]
#[test]
fn reuse_cached_engine_rejects_a_wrapper_that_escapes_the_slot() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let outside_wrapper = outside.path().join("exe");
    fs::create_dir_all(&outside_wrapper).expect("create outside wrapper");
    fs::write(outside_wrapper.join("package.json"), r#"{"name":"@pnpm/exe","version":"11.10.0"}"#)
        .expect("write outside wrapper manifest");

    fs::create_dir_all(
        temp.path()
            .join("node_modules")
            .join("@pnpm"),
    )
    .expect("create scope dir");
    std::os::unix::fs::symlink(&outside_wrapper, package_dir(temp.path(), PNPM_EXE_PACKAGE_NAME))
        .expect("symlink wrapper outside slot");

    // The recorded version matches, but the wrapper resolves outside the
    // slot, so the slot is not reusable.
    assert!(!reuse_cached_engine(temp.path(), pnpm_package_to_install("11.10.0"), "11.10.0"));
}

/// Alpine arm64 loads neither published native build. The glibc binary's
/// interpreter is missing, and the musl arm64 build needs `libc.so`.
/// The wrapper then runs `dist/pnpm.mjs` with Node.js.
#[cfg(unix)]
#[test]
fn uses_the_javascript_build_when_no_native_binary_can_load() {
    assert!(which::which("node").is_ok(), "node is on PATH");
    let temp = tempfile::tempdir().expect("tempdir");
    let platform_dir = exe_platform_pkg_dir_name_next(host_platform(), host_arch(), host_libc());
    write_platform_binary(temp.path(), PNPM_EXE_PACKAGE_NAME, &platform_dir, &unloadable_elf());
    let wrapper = package_dir(temp.path(), PNPM_EXE_PACKAGE_NAME);
    fs::create_dir_all(wrapper.join("dist")).expect("create dist");
    fs::write(
        wrapper.join("dist").join("pnpm.mjs"),
        "import { writeSync } from 'node:fs'\nwriteSync(1, 'from-js\\n')\n",
    )
    .expect("write js build");

    link_exe_platform_binary(temp.path(), PNPM_EXE_PACKAGE_NAME).expect("link");
    link_exe_platform_binary(temp.path(), PNPM_EXE_PACKAGE_NAME).expect("relink");

    let linked = fs::read_to_string(wrapper.join("pnpm")).expect("read launcher");
    let node = node_executable(std::env::var_os("PATH").as_deref()).expect("node");
    let quoted = pnpm_cmd_shim::sh_single_quote(node.to_str().expect("utf8 node"));
    assert!(linked.starts_with("#!/bin/sh\n"), "{linked}");
    assert!(linked.contains("/dist/pnpm.mjs"), "{linked}");
    assert!(linked.contains(&format!("exec {quoted} ")), "{linked}");
    assert!(
        fs::read_dir(&wrapper)
            .expect("read wrapper")
            .flatten()
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".pnpm-js-launcher.")),
        "the launcher temp file is removed after rename"
    );
    let platform_path = temp
        .path()
        .join("node_modules")
        .join("@pnpm")
        .join(&platform_dir);
    assert!(
        fs::read_dir(&platform_path)
            .expect("read platform dir")
            .flatten()
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".pnpm-bin-probe.")),
        "the execution probe is removed"
    );

    let bin_dir = temp.path().join("bin");
    fs::create_dir(&bin_dir).expect("create bin");
    std::os::unix::fs::symlink("../node_modules/@pnpm/exe/pnpm", bin_dir.join("pnpm"))
        .expect("symlink bin");
    let output = std::process::Command::new(bin_dir.join("pnpm")).output().expect("run launcher");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.stdout, b"from-js\n");
}

/// A v12 package has no JavaScript twin. An unloadable native binary is
/// still what gets linked, so the failure stays visible.
#[cfg(unix)]
#[test]
fn links_an_unloadable_binary_when_the_javascript_build_is_absent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let platform_dir = exe_platform_pkg_dir_name_next(host_platform(), host_arch(), host_libc());
    let elf = unloadable_elf();
    write_platform_binary(temp.path(), PNPM_PACKAGE_NAME, &platform_dir, &elf);

    link_exe_platform_binary(temp.path(), PNPM_PACKAGE_NAME).expect("link");

    let linked = fs::read(package_dir(temp.path(), PNPM_PACKAGE_NAME).join("pnpm"))
        .expect("read linked binary");
    assert!(linked.starts_with(b"\x7fELF"), "the native binary is linked when no JS build exists");
}

/// On Linux the other libc's package is a candidate. A runnable sibling is
/// linked even when the preferred binary cannot be loaded.
#[cfg(target_os = "linux")]
#[test]
fn prefers_a_runnable_libc_sibling_over_an_unloadable_binary() {
    let temp = tempfile::tempdir().expect("tempdir");
    let platform = host_platform();
    let arch = host_arch();
    let libc = host_libc();
    let primary = exe_platform_pkg_dir_name(platform, arch, libc);
    let alternate_libc = if libc == "musl" { "" } else { "musl" };
    let alternate = exe_platform_pkg_dir_name(platform, arch, alternate_libc);
    write_platform_binary(temp.path(), PNPM_EXE_PACKAGE_NAME, &primary, &unloadable_elf());
    let script = b"#!/bin/sh\necho runnable-sibling\n";
    write_platform_binary(temp.path(), PNPM_EXE_PACKAGE_NAME, &alternate, script);
    let wrapper = package_dir(temp.path(), PNPM_EXE_PACKAGE_NAME);
    fs::create_dir_all(wrapper.join("dist")).expect("create dist");
    fs::write(wrapper.join("dist").join("pnpm.mjs"), "export {}\n").expect("write js build");

    link_exe_platform_binary(temp.path(), PNPM_EXE_PACKAGE_NAME).expect("link");

    assert_eq!(fs::read(wrapper.join("pnpm")).expect("read linked binary"), script);
}

#[test]
fn launcher_temp_paths_do_not_collide() {
    let temp = tempfile::tempdir().expect("tempdir");
    let first = launcher_temp_path(temp.path());
    let second = launcher_temp_path(temp.path());
    assert_ne!(first, second);
}

#[test]
fn node_executable_requires_an_absolute_executable_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    assert!(node_executable(None).is_none());
    assert!(node_executable(Some(std::ffi::OsStr::new(""))).is_none());
    assert!(node_executable(Some(temp.path().as_os_str())).is_none());

    let dir_named_node = temp.path().join("node");
    fs::create_dir(&dir_named_node).expect("directory named node");
    assert!(node_executable(Some(temp.path().as_os_str())).is_none());
    fs::remove_dir(&dir_named_node).expect("remove directory");

    let node = temp.path().join("node");
    fs::write(&node, "#!/bin/sh\n").expect("write node");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&node, fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(node_executable(Some(temp.path().as_os_str())).is_none());
        fs::set_permissions(&node, fs::Permissions::from_mode(0o755)).expect("chmod executable");
        let found = node_executable(Some(temp.path().as_os_str())).expect("executable node");
        assert_eq!(found, node);
    }
}

#[test]
fn the_javascript_launcher_quotes_the_node_path() {
    let script =
        node_launcher_script(std::path::Path::new("/opt/node's/bin/node")).expect("script");
    assert!(script.contains("exec '/opt/node'\\''s/bin/node' "), "{script}");
    assert!(!script.contains("exec /opt/node's/bin/node"), "{script}");
    assert!(node_launcher_script(std::path::Path::new("/opt/node\n/bin/node")).is_none());
    assert!(node_launcher_script(std::path::Path::new("/opt/node\r/bin/node")).is_none());
}

#[cfg(unix)]
#[test]
fn node_executable_skips_a_path_containing_a_newline() {
    let parent = tempfile::tempdir().expect("tempdir");
    let dir = parent.path().join("a\nb");
    fs::create_dir(&dir).expect("dir with a newline");
    let node = dir.join("node");
    fs::write(&node, "#!/bin/sh\n").expect("write node");
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&node, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    assert!(node_executable(Some(dir.as_os_str())).is_none());
}

#[cfg(unix)]
fn write_platform_binary(
    install_dir: &std::path::Path,
    wrapper_pkg_name: &str,
    platform_dir: &str,
    bytes: &[u8],
) {
    fake_engine_install_for(install_dir, wrapper_pkg_name, false);
    let src_dir = install_dir
        .join("node_modules")
        .join("@pnpm")
        .join(platform_dir);
    fs::create_dir_all(&src_dir).expect("create platform dir");
    fs::write(src_dir.join("pnpm"), bytes).expect("write platform binary");
}

#[cfg(unix)]
fn unloadable_elf() -> Vec<u8> {
    let interp = b"/no/such/pnpm-ld-10443.so\0";
    let interp_off = 64 + 56;
    let mut elf = vec![0_u8; interp_off + interp.len()];
    elf[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    elf[4] = 2;
    elf[5] = 1;
    elf[18..20].copy_from_slice(&0xffff_u16.to_le_bytes());
    elf[32..40].copy_from_slice(&64_u64.to_le_bytes());
    elf[54..56].copy_from_slice(&56_u16.to_le_bytes());
    elf[56..58].copy_from_slice(&1_u16.to_le_bytes());
    elf[64..68].copy_from_slice(&3_u32.to_le_bytes());
    elf[72..80].copy_from_slice(&u64::try_from(interp_off).unwrap().to_le_bytes());
    elf[96..104].copy_from_slice(&u64::try_from(interp.len()).unwrap().to_le_bytes());
    elf[interp_off..].copy_from_slice(interp);
    elf
}
