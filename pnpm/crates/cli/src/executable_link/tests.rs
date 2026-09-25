use super::replace_executable;
use std::{fs, io, path::Path};
use tempfile::tempdir;

fn write_executable(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().expect("executable has a parent")).expect("create parent");
    fs::write(path, contents).expect("write executable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("mark executable");
    }
}

/// Homebrew's `pnpm` is a relative symlink into Cellar. Publishing that
/// entry with `hard_link` would copy the symlink, which does not resolve
/// from the global bin directory.
#[cfg(unix)]
#[test]
fn a_relative_symlink_publishes_a_hard_link_of_the_resolved_binary() {
    use std::os::unix::fs::symlink;

    let root = tempdir().expect("create temp dir");
    let binary = root.path().join("opt/homebrew/Cellar/pnpm/12.6.0/bin/pnpm");
    write_executable(&binary, b"pnpm-binary");

    let brew_link = root.path().join("opt/homebrew/bin/pnpm");
    fs::create_dir_all(brew_link.parent().expect("brew bin parent")).expect("create brew bin");
    let relative_target = Path::new("../Cellar/pnpm/12.6.0/bin/pnpm");
    symlink(relative_target, &brew_link).expect("link Homebrew pnpm");

    let shim = root.path().join("Library/pnpm/bin/node");
    let shim_dir = shim.parent().expect("global bin parent");
    fs::create_dir_all(shim_dir).expect("create global bin");
    let dangling_from_shim = shim_dir.join(relative_target);
    eprintln!("cellar-relative path from the shim dir: {}", dangling_from_shim.display());
    assert!(
        !dangling_from_shim.exists(),
        "the Cellar-relative target must not resolve from the global bin directory",
    );

    replace_executable(&brew_link, &shim).expect("publish the shim");

    let published = fs::symlink_metadata(&shim).expect("stat the shim");
    eprintln!(
        "published symlink={} file={}",
        published.file_type().is_symlink(),
        published.file_type().is_file(),
    );
    assert!(published.file_type().is_file(), "the shim must not be a symlink");
    let same = same_file::is_same_file(&binary, &shim).expect("compare shim and binary");
    eprintln!("shim and resolved binary are the same file: {same}");
    assert!(same, "the shim is a hard link of the resolved binary");
    assert_eq!(fs::read(&shim).expect("read shim"), b"pnpm-binary");
    assert_eq!(fs::read_link(&brew_link).expect("read Homebrew link"), relative_target);
}

/// A shim already published as a hard link of the Homebrew symlink is the
/// broken on-disk state. Republishing must replace it with the binary.
#[cfg(unix)]
#[test]
fn republishing_replaces_a_hard_link_of_the_relative_symlink() {
    use std::os::unix::fs::symlink;

    let root = tempdir().expect("create temp dir");
    let binary = root.path().join("opt/homebrew/Cellar/pnpm/12.6.0/bin/pnpm");
    write_executable(&binary, b"pnpm-binary");

    let brew_link = root.path().join("opt/homebrew/bin/pnpm");
    fs::create_dir_all(brew_link.parent().expect("brew bin parent")).expect("create brew bin");
    symlink("../Cellar/pnpm/12.6.0/bin/pnpm", &brew_link).expect("link Homebrew pnpm");

    let shim = root.path().join("Library/pnpm/bin/node");
    fs::create_dir_all(shim.parent().expect("global bin parent")).expect("create global bin");
    fs::hard_link(&brew_link, &shim).expect("reproduce the dangling shim");
    let dangling = fs::symlink_metadata(&shim).expect("stat dangling shim");
    eprintln!("dangling shim is a symlink: {}", dangling.file_type().is_symlink());
    assert!(dangling.file_type().is_symlink(), "the reproduced shim is the Homebrew symlink");

    replace_executable(&brew_link, &shim).expect("republish the shim");

    let published = fs::symlink_metadata(&shim).expect("stat the shim");
    eprintln!(
        "republished symlink={} file={}",
        published.file_type().is_symlink(),
        published.file_type().is_file(),
    );
    assert!(published.file_type().is_file(), "republishing must replace the symlink");
    let same = same_file::is_same_file(&binary, &shim).expect("compare shim and binary");
    eprintln!("shim and resolved binary are the same file: {same}");
    assert!(same, "the replacement is a hard link of the resolved binary");
    assert_eq!(fs::read(&shim).expect("read shim"), b"pnpm-binary");
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_is_not_published() {
    use std::os::unix::fs::symlink;

    let root = tempdir().expect("create temp dir");
    let link = root.path().join("pnpm");
    symlink("../Cellar/pnpm/missing/bin/pnpm", &link).expect("link a missing binary");
    let shim = root.path().join("Library/pnpm/bin/node");
    fs::create_dir_all(shim.parent().expect("global bin parent")).expect("create global bin");

    let error = replace_executable(&link, &shim).expect_err("dangling symlink must fail");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    eprintln!("shim exists after a dangling source: {}", shim.exists());
    assert!(!shim.exists(), "a dangling symlink must not be published as the shim");
}

#[test]
fn an_executable_file_is_published_as_a_hard_link() {
    let root = tempdir().expect("create temp dir");
    let source = root.path().join("pnpm");
    write_executable(&source, b"pnpm-binary");
    let shim = root.path().join("bin/node");
    fs::create_dir_all(shim.parent().expect("shim parent")).expect("create bin");

    replace_executable(&source, &shim).expect("publish the shim");

    let same = same_file::is_same_file(&source, &shim).expect("compare shim and source");
    eprintln!("shim and source are the same file: {same}");
    assert!(same, "same filesystem publishes a hard link");
    assert_eq!(fs::read(&shim).expect("read shim"), b"pnpm-binary");
}
