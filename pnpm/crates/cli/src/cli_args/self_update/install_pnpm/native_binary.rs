use super::{
    Context, IntoDiagnostic, Path, PathBuf, Value, format_global_virtual_store_path, fs, host_arch,
    host_libc, host_platform, package_dir, parse_manifest, replace_executable,
};
use std::{
    ffi::OsStr,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// Scope-local directory name of the `@pnpm/exe` platform package under
/// the legacy `<os>-<arch>` scheme (`macos-arm64`, `win-x86`,
/// `linux-x64`, `linuxstatic-x64`).
pub(in super::super) fn exe_platform_pkg_dir_name(
    platform: &str,
    arch: &str,
    libc: &str,
) -> String {
    let arch = normalized_arch(platform, arch);
    let os = match platform {
        "darwin" => "macos",
        "win32" => "win",
        "linux" => {
            if libc == "musl" {
                "linuxstatic"
            } else {
                "linux"
            }
        }
        other => other,
    };
    format!("{os}-{arch}")
}

/// Scope-local directory name of the platform package under the
/// `exe.<platform>-<arch>[-musl]` scheme — the convention pnpm v12 ships
/// its native binaries under.
pub(in super::super) fn exe_platform_pkg_dir_name_next(
    platform: &str,
    arch: &str,
    libc: &str,
) -> String {
    format!("exe.{}", native_target_name(platform, arch, libc))
}

/// The `<platform>-<arch>[-musl]` target a pnpm native binary is built for,
/// as the `exe.<target>` platform packages are named after it.
pub(in super::super) fn native_target_name(platform: &str, arch: &str, libc: &str) -> String {
    let arch = normalized_arch(platform, arch);
    let libc_suffix = if platform == "linux" && libc == "musl" { "-musl" } else { "" };
    format!("{platform}-{arch}{libc_suffix}")
}

fn normalized_arch<'a>(platform: &str, arch: &'a str) -> &'a str {
    if platform == "win32" && arch == "ia32" { "x86" } else { arch }
}

/// Link the host's native platform binary (`@pnpm/exe.<target>`) into the
/// wrapper package directory, replicating the wrapper's preinstall step
/// (skipped because the engine is installed with scripts disabled).
///
/// Errors loudly when the wrapper or its platform binary is missing, or
/// when the hard link fails: with scripts disabled, this manual linking is
/// the critical path, so a silent no-op would leave a "successful"
/// self-update with a non-functional `pnpm`.
pub(crate) fn link_exe_platform_binary(
    install_dir: &Path,
    wrapper_pkg_name: &str,
) -> miette::Result<()> {
    let wrapper_dir = package_dir(install_dir, wrapper_pkg_name);
    if !wrapper_dir.exists() {
        let wrapper_display = wrapper_dir.display();
        return Err(miette::miette!("the installed pnpm wrapper is missing at {wrapper_display}"));
    }
    let platform = host_platform();
    let executable = if platform == "win32" { "pnpm.exe" } else { "pnpm" };

    let (install_real_dir, wrapper_real_dir) = canonical_wrapper_dirs(install_dir, &wrapper_dir)?;
    let parent = wrapper_real_dir
        .parent()
        .ok_or_else(|| miette::miette!("the pnpm wrapper has no parent directory"))?;
    let scope_dir =
        if wrapper_pkg_name.starts_with('@') { parent.to_path_buf() } else { parent.join("@pnpm") };

    let native_source_root = native_source_trust_root(&install_real_dir, wrapper_pkg_name);
    let sources = validated_native_binaries(&scope_dir, platform, executable, &native_source_root)?;
    let dest = wrapper_real_dir.join(executable);
    if let Some(src) = sources.iter().find(|src| binary_runs(src)) {
        publish_native_binary(src, &dest, platform, &wrapper_real_dir)?;
        return Ok(());
    }
    // Alpine arm64 cannot load the published native builds: a static pnpm
    // does not identify itself as musl, so the glibc binary is the first
    // candidate, and `@pnpm/linuxstatic-arm64` needs `libc.so`, which Alpine
    // does not provide. `@pnpm/exe` ships the JavaScript CLI next to that
    // binary, and Node.js is already on PATH in the image where this fails.
    if publish_node_launcher(&wrapper_real_dir, &dest)? {
        return Ok(());
    }
    let src = sources
        .first()
        .ok_or_else(|| miette::miette!("no @pnpm/exe native binary was found for this host"))?;
    publish_native_binary(src, &dest, platform, &wrapper_real_dir)
}

fn publish_native_binary(
    src: &Path,
    dest: &Path,
    platform: &str,
    wrapper_real_dir: &Path,
) -> miette::Result<()> {
    replace_executable(src, dest)
        .into_diagnostic()
        .wrap_err("link the native pnpm binary into the wrapper")?;
    if platform == "win32" {
        link_windows_aliases(src, wrapper_real_dir)?;
        rewrite_windows_bin_field(wrapper_real_dir);
    }
    Ok(())
}

/// True when this host can start `path`. A mode without the execute bit is
/// copied to a unique file beside it and marked executable first, because the
/// store blob is often not executable until it is linked. The copy is removed
/// before this returns. A binary that fails to load exits non-zero here, which
/// is how a glibc build and the musl arm64 build that needs `libc.so` are
/// skipped on Alpine.
fn binary_runs(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    if is_executable(path) {
        return command_succeeded(path);
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    let staged = probe_temp_path(parent);
    let staged_ok = (|| -> std::io::Result<()> {
        fs::copy(path, &staged)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    })();
    if staged_ok.is_err() {
        let _ = fs::remove_file(&staged);
        return false;
    }
    let ok = command_succeeded(&staged);
    let _ = fs::remove_file(&staged);
    ok
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

fn probe_temp_path(dir: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    dir.join(format!(
        ".pnpm-bin-probe.{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ))
}

fn command_succeeded(path: &Path) -> bool {
    let Ok(mut child) = std::process::Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Write a Node launcher over the wrapper bin when `dist/pnpm.mjs` is part
/// of this package and `node` is an absolute executable on `PATH`. Returns
/// `false` when that JavaScript build is not there to run.
///
/// The script names that absolute `node`. It does not search `PATH` again
/// when the bin runs, so a later `PATH` entry cannot replace the interpreter.
fn publish_node_launcher(wrapper_real_dir: &Path, dest: &Path) -> miette::Result<bool> {
    if host_platform() == "win32" {
        return Ok(false);
    }
    let js_entry = wrapper_real_dir.join("dist").join("pnpm.mjs");
    if !js_entry.is_file() {
        return Ok(false);
    }
    let Some(node) = node_executable(std::env::var_os("PATH").as_deref()) else {
        return Ok(false);
    };
    let Some(script) = node_launcher_script(&node) else {
        return Ok(false);
    };
    // `bin/pnpm` is a symlink to this file. The script resolves `$0` before
    // locating `dist/pnpm.mjs` next to the real file.
    let temp = launcher_temp_path(wrapper_real_dir);
    let published = (|| {
        fs::write(&temp, &script)
            .into_diagnostic()
            .wrap_err("write the JavaScript pnpm launcher")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o755))
                .into_diagnostic()
                .wrap_err("mark the JavaScript pnpm launcher executable")?;
        }
        replace_executable(&temp, dest)
            .into_diagnostic()
            .wrap_err("install the JavaScript pnpm launcher")
    })();
    let _ = fs::remove_file(&temp);
    published?;
    Ok(true)
}

/// A temp path in `dir` that no other in-process publish shares.
pub(crate) fn launcher_temp_path(dir: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    dir.join(format!(
        ".pnpm-js-launcher.{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ))
}

/// The `node` executable named by `path_env`, or `None` when that search
/// does not find an absolute path. A directory or a non-executable file
/// named `node` does not count. A path that cannot be embedded in the
/// launcher script does not count either.
pub(crate) fn node_executable(path_env: Option<&OsStr>) -> Option<PathBuf> {
    let path_env = path_env?;
    if path_env.is_empty() {
        return None;
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let node = which::which_in("node", Some(path_env), &cwd).ok()?;
    if !node.is_absolute() {
        return None;
    }
    let text = node.to_str()?;
    if text.contains('\n') || text.contains('\r') {
        return None;
    }
    Some(node)
}

/// Shell script that runs `dist/pnpm.mjs` with `node`. `node` is embedded
/// as a single-quoted absolute path.
pub(crate) fn node_launcher_script(node: &Path) -> Option<String> {
    let text = node.to_str()?;
    if text.contains('\n') || text.contains('\r') {
        return None;
    }
    let quoted = pnpm_cmd_shim::sh_single_quote(text);
    Some(format!(
        "#!/bin/sh\n\
target=$0\n\
while [ -L \"$target\" ]; do\n\
  parent=$(CDPATH= cd -- \"$(dirname \"$target\")\" && pwd) || exit 1\n\
  link=$(readlink \"$target\") || exit 1\n\
  case $link in\n\
    /*) target=$link ;;\n\
    *) target=\"$parent/$link\" ;;\n\
  esac\n\
done\n\
dir=$(CDPATH= cd -- \"$(dirname \"$target\")\" && pwd) || exit 1\n\
exec {quoted} \"$dir/dist/pnpm.mjs\" \"$@\"\n"
    ))
}

/// Resolve the platform binary by its explicit adjacent path in the
/// real virtual store, not via a `node_modules` walk (which a
/// repo-controlled store-dir could shadow). `@pnpm/exe`'s parent is
/// already `@pnpm`; the unscoped `pnpm` descends into `@pnpm`.
fn canonical_wrapper_dirs(
    install_dir: &Path,
    wrapper_dir: &Path,
) -> miette::Result<(PathBuf, PathBuf)> {
    let install_real_dir = fs::canonicalize(install_dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("resolve the pnpm install dir at {}", install_dir.display()))?;
    let wrapper_real_dir = fs::canonicalize(wrapper_dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("resolve the pnpm wrapper at {}", wrapper_dir.display()))?;
    if !wrapper_real_dir.starts_with(&install_real_dir) {
        let wrapper_display = wrapper_dir.display();
        let install_display = install_dir.display();
        return Err(miette::miette!(
            "the installed pnpm wrapper at {} resolves outside {}",
            wrapper_display,
            install_display
        ));
    }
    Ok((install_real_dir, wrapper_real_dir))
}

/// Platform binaries under `scope_dir`, preferred libc first and the other
/// Linux libc after it. An unknown libc is not musl, so the glibc package
/// is first; the musl package is still a candidate when that binary cannot
/// be loaded.
fn validated_native_binaries(
    scope_dir: &Path,
    platform: &str,
    executable: &str,
    source_root: &Path,
) -> miette::Result<Vec<PathBuf>> {
    let arch = host_arch();
    let libc = host_libc();
    let mut sources = Vec::new();
    for dir_name in platform_binary_dir_names(platform, arch, libc) {
        let candidate = scope_dir.join(dir_name).join(executable);
        if !candidate.exists() {
            continue;
        }
        sources.push(validate_native_binary_source(&candidate, source_root)?);
    }
    if sources.is_empty() {
        return Err(miette::miette!(
            "no @pnpm/exe.{platform}-{arch} native binary was found for this host"
        ));
    }
    Ok(sources)
}

fn platform_binary_dir_names(platform: &str, arch: &str, libc: &str) -> Vec<String> {
    let mut names = vec![
        exe_platform_pkg_dir_name(platform, arch, libc),
        exe_platform_pkg_dir_name_next(platform, arch, libc),
    ];
    if platform == "linux" {
        let alternate = if libc == "musl" { "" } else { "musl" };
        for name in [
            exe_platform_pkg_dir_name(platform, arch, alternate),
            exe_platform_pkg_dir_name_next(platform, arch, alternate),
        ] {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// Aliases (pn / pnpx / pnx) must be .exe hardlinks of the native
/// binary, not .cmd wrappers — cmd-shim's Bash shim mangles a .cmd
/// target under MSYS2 / Git Bash. The native binary detects which
/// name it was launched as and prepends `dlx` for pnpx / pnx.
fn link_windows_aliases(src: &Path, wrapper_real_dir: &Path) -> miette::Result<()> {
    for alias in ["pn", "pnpx", "pnx"] {
        replace_executable(src, &wrapper_real_dir.join(format!("{alias}.exe")))
            .into_diagnostic()
            .wrap_err_with(|| format!("link the {alias} alias into the wrapper"))?;
    }
    Ok(())
}

fn native_source_trust_root(install_real_dir: &Path, wrapper_pkg_name: &str) -> PathBuf {
    // In the global virtual store, the wrapper and platform binary live
    // in sibling slots under `links`; self-update installs keep both
    // under the one install dir.
    global_virtual_store_root_from_slot(install_real_dir, wrapper_pkg_name)
        .unwrap_or_else(|| install_real_dir.to_path_buf())
}

// Recognizes a slot by re-deriving its `links`-relative path with
// [`format_global_virtual_store_path`] — the same formatter that laid the
// slot out — so this walk can't drift from the layout (e.g. the `@`
// placeholder scope segment unscoped packages sit under).
fn global_virtual_store_root_from_slot(slot_dir: &Path, package_name: &str) -> Option<PathBuf> {
    let hash = slot_dir.file_name()?.to_str()?;
    let version = slot_dir
        .parent()?
        .file_name()?
        .to_str()?;
    node_semver::Version::parse(version).ok()?;

    let mut cursor = slot_dir;
    for segment in format_global_virtual_store_path(package_name, version, hash).split('/').rev() {
        if cursor.file_name()?.to_str()? != segment {
            return None;
        }
        cursor = cursor.parent()?;
    }
    (cursor.file_name()?.to_str()? == "links").then(|| cursor.to_path_buf())
}

fn validate_native_binary_source(src: &Path, source_root: &Path) -> miette::Result<PathBuf> {
    let src_display = src.display().to_string();
    let link_meta = fs::symlink_metadata(src)
        .into_diagnostic()
        .wrap_err_with(|| format!("inspect the native pnpm binary at {src_display}"))?;
    if link_meta.file_type().is_symlink() {
        return Err(miette::miette!("the native pnpm binary at {src_display} is a symlink"));
    }
    let src_real = fs::canonicalize(src)
        .into_diagnostic()
        .wrap_err_with(|| format!("resolve the native pnpm binary at {src_display}"))?;
    if !src_real.starts_with(source_root) {
        let source_root_display = source_root.display().to_string();
        return Err(miette::miette!(
            "the native pnpm binary at {src_display} resolves outside {source_root_display}"
        ));
    }
    let src_real_display = src_real.display().to_string();
    let meta = fs::metadata(&src_real)
        .into_diagnostic()
        .wrap_err_with(|| format!("inspect the native pnpm binary at {src_real_display}"))?;
    if !meta.is_file() {
        return Err(miette::miette!(
            "the native pnpm binary at {src_real_display} is not a regular file"
        ));
    }
    Ok(src_real)
}

/// Point the Windows wrapper's `bin` field at the `.exe` variants (the
/// npm shim generator reads `bin` at install time). Written via a temp
/// file + rename so the content-addressed, hard-linked `package.json`
/// blob is not mutated in place.
fn rewrite_windows_bin_field(wrapper_dir: &Path) {
    let pkg_json_path = wrapper_dir.join("package.json");
    let Ok(text) = fs::read_to_string(&pkg_json_path) else {
        return;
    };
    let Ok(mut pkg) = parse_manifest(&text) else {
        return;
    };
    let Some(bin) = pkg.get_mut("bin").and_then(Value::as_object_mut) else {
        return;
    };
    for (name, target) in
        [("pnpm", "pnpm.exe"), ("pn", "pn.exe"), ("pnpx", "pnpx.exe"), ("pnx", "pnx.exe")]
    {
        bin.insert(name.to_string(), Value::String(target.to_string()));
    }
    let Ok(serialized) = serde_json::to_string_pretty(&pkg) else {
        return;
    };
    let temp_path = pkg_json_path.with_extension("json.pnpm-tmp");
    if fs::write(&temp_path, serialized).is_err() {
        let _ = fs::remove_file(&temp_path);
        return;
    }
    if fs::rename(&temp_path, &pkg_json_path).is_err() {
        let _ = fs::remove_file(&temp_path);
    }
}
