use super::{
    Arc, Host, LinkBinsOptions, PackageBinSource, create_dir_all, json, link_bins_of_packages,
    tempdir, write_file,
};

#[test]
fn same_package_bin_conflict_is_an_error() {
    let tmp = tempdir().unwrap();
    let modules = tmp.path().join("node_modules");
    let packages = [("prettier-2", "2.8.8"), ("prettier", "3.0.3")].map(|(alias, version)| {
        let location = modules.join(alias);
        create_dir_all(&location).unwrap();
        write_file(location.join("index.js"), "#!/usr/bin/env node\n").unwrap();
        PackageBinSource::new(
            location,
            Arc::new(json!({
                "name": "prettier",
                "version": version,
                "bin": { "prettier": "index.js" },
            })),
        )
    });
    let bins = modules.join(".bin");

    let err = link_bins_of_packages::<Host>(&packages, &bins, &LinkBinsOptions::default())
        .expect_err("two owners of prettier must not link");

    let message = err.to_string();
    eprintln!("conflict message: {message}");
    assert_eq!(
        miette::Diagnostic::code(&err).map(|code| code.to_string()).as_deref(),
        Some("ERR_PNPM_BINARIES_CONFLICT"),
    );
    assert!(
        message.contains("prettier-2") && message.contains("prettier@3.0.3"),
        "the error must name both providers, got {message}",
    );
    assert!(!bins.join("prettier").exists(), "a colliding bin must not be written");
}

#[test]
fn distinct_bin_names_both_link() {
    let tmp = tempdir().unwrap();
    let modules = tmp.path().join("node_modules");
    let packages = [("left-tool", "left"), ("right-tool", "right")].map(|(name, bin)| {
        let location = modules.join(name);
        create_dir_all(&location).unwrap();
        write_file(location.join("index.js"), "#!/usr/bin/env node\n").unwrap();
        PackageBinSource::new(
            location,
            Arc::new(json!({
                "name": name,
                "version": "1.0.0",
                "bin": { bin: "index.js" },
            })),
        )
    });
    let bins = modules.join(".bin");

    link_bins_of_packages::<Host>(&packages, &bins, &LinkBinsOptions::default())
        .expect("bins that do not share a name link");

    assert!(bins.join("left").exists(), "left bin is linked");
    assert!(bins.join("right").exists(), "right bin is linked");
}
