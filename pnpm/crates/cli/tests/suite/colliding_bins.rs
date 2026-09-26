use crate::_utils::pacquet_in;

use assert_cmd::prelude::*;
use std::{fs, path::Path};
use tempfile::tempdir;

fn write_bin_package(dir: &Path, name: &str, version: &str, bin: &str) {
    fs::create_dir_all(dir).expect("create package dir");
    fs::write(
        dir.join("package.json"),
        serde_json::json!({
            "name": name,
            "version": version,
            "bin": { bin: "index.js" },
        })
        .to_string(),
    )
    .expect("write package.json");
    fs::write(dir.join("index.js"), "#!/usr/bin/env node\n").expect("write bin script");
}

#[test]
fn install_fails_when_two_dependencies_provide_the_same_bin() {
    let root = tempdir().expect("create temp directory");
    let workspace = root.path();
    write_bin_package(&workspace.join("prettier"), "prettier", "3.0.3", "prettier");
    write_bin_package(&workspace.join("prettier-2"), "prettier", "2.8.8", "prettier");
    fs::write(
        workspace.join("package.json"),
        serde_json::json!({
            "name": "app",
            "private": true,
            "dependencies": {
                "prettier": "file:prettier",
                "prettier-2": "file:prettier-2",
            },
        })
        .to_string(),
    )
    .expect("write project manifest");

    let assert = pacquet_in(workspace)
        .arg("install")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("ERR_PNPM_BINARIES_CONFLICT"),
        "install must fail on the colliding prettier bin, stderr:\n{stderr}",
    );
    assert!(
        !workspace.join("node_modules/.bin/prettier").exists(),
        "the colliding bin must not be linked",
    );
}

#[test]
fn install_links_bins_that_do_not_collide() {
    let root = tempdir().expect("create temp directory");
    let workspace = root.path();
    write_bin_package(&workspace.join("left-tool"), "left-tool", "1.0.0", "left");
    write_bin_package(&workspace.join("right-tool"), "right-tool", "1.0.0", "right");
    fs::write(
        workspace.join("package.json"),
        serde_json::json!({
            "name": "app",
            "private": true,
            "dependencies": {
                "left-tool": "file:left-tool",
                "right-tool": "file:right-tool",
            },
        })
        .to_string(),
    )
    .expect("write project manifest");

    pacquet_in(workspace)
        .arg("install")
        .assert()
        .success();

    assert!(workspace.join("node_modules/.bin/left").exists(), "left bin is linked");
    assert!(workspace.join("node_modules/.bin/right").exists(), "right bin is linked");
}
