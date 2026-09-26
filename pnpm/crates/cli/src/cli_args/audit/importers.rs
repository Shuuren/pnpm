//! The lockfile importers `audit` and `audit signatures` cover.

use super::{
    AuditError, AuditIndexRequest, EnvLockfile, HashMap, Include, Lockfile, State,
    UnresolvableLockfileDependency, lockfile_to_audit_request, pick_registry_for_package,
    signatures,
};
use crate::cli_args::recursive::{
    no_projects_matched_message, notice_workspace_dir, selected_workspace_importer_ids,
    selectors_narrow_the_run,
};
use crate::cli_args::sanitize::sanitize_inline;
use std::borrow::Cow;

/// The lockfile narrowed to the importers of the projects that `--filter`,
/// `--filter-prod`, or `--workspace-root` selected, or the whole lockfile
/// when no selector narrows the run. `None` when the selectors matched no
/// project, after printing pnpm's notice for it. A selected project without
/// an importer entry is an error: auditing the rest would report it clean.
pub(super) fn select_audited_importers<'lockfile>(
    state: &State,
    lockfile: &'lockfile Lockfile,
) -> miette::Result<Option<Cow<'lockfile, Lockfile>>> {
    if !selectors_narrow_the_run(state.config) {
        return Ok(Some(Cow::Borrowed(lockfile)));
    }
    let selected =
        selected_workspace_importer_ids(state.config, state.project_dir(), state.lockfile_dir())?;
    if selected.is_empty() {
        let workspace_dir = notice_workspace_dir(state.config, state.project_dir());
        println!("{}", no_projects_matched_message(workspace_dir));
        return Ok(None);
    }
    let mut missing: Vec<&str> = selected
        .iter()
        .map(String::as_str)
        .filter(|importer_id| !lockfile.importers.contains_key(*importer_id))
        .collect();
    if !missing.is_empty() {
        missing.sort_unstable();
        return Err(AuditError::MissingImporters { importer_ids: missing.join(", ") }.into());
    }
    let mut narrowed = lockfile.clone();
    narrowed.importers.retain(|importer_id, _| selected.contains(importer_id));
    Ok(Some(Cow::Owned(narrowed)))
}

/// Installed packages to verify, plus lockfile references that have no snapshot.
pub(super) struct SignatureAuditSet {
    pub(super) packages: Vec<signatures::SignaturePackage>,
    pub(super) unresolvable: Vec<signatures::SignatureIssue>,
}

/// Every installed package version the lockfile and env lockfile record,
/// with the registry that serves it. `None` when the selectors matched no
/// project. Unresolvable references are returned beside the packages so the
/// signature audit can fail closed without asking the registry about them.
pub(super) fn signature_packages(
    state: &State,
    include: Include,
    lockfile_dir: &std::path::Path,
) -> miette::Result<Option<SignatureAuditSet>> {
    let lockfile = state.lockfile
        .get()
        .map_err(|err| miette::Report::new(err).wrap_err("load the lockfile"))?;
    let Some(lockfile) = lockfile else {
        return Err(AuditError::NoLockfile.into());
    };
    let Some(lockfile) = select_audited_importers(state, lockfile)? else {
        return Ok(None);
    };
    let lockfile = lockfile.as_ref();
    let env_lockfile = EnvLockfile::read(lockfile_dir)
        .map_err(|err| miette::Report::new(err).wrap_err("load the env lockfile"))?;
    let audit_request = lockfile_to_audit_request(lockfile, env_lockfile.as_ref(), include);
    let registries: HashMap<String, String> = state.config
        .resolved_registries()
        .into_iter()
        .collect();
    Ok(Some(SignatureAuditSet {
        packages: packages_from_request(&audit_request, &registries),
        unresolvable: unresolvable_signature_issues(&audit_request, &registries),
    }))
}

fn packages_from_request(
    audit_request: &AuditIndexRequest,
    registries: &HashMap<String, String>,
) -> Vec<signatures::SignaturePackage> {
    audit_request.request
        .iter()
        .flat_map(|(name, versions)| {
            let registry = pick_registry_for_package(registries, name, None);
            versions
                .iter()
                .map(move |version| signatures::SignaturePackage {
                    name: name.clone(),
                    registry: registry.clone(),
                    version: version.clone(),
                })
        })
        .collect()
}

fn unresolvable_signature_issues(
    audit_request: &AuditIndexRequest,
    registries: &HashMap<String, String>,
) -> Vec<signatures::SignatureIssue> {
    audit_request.unresolvable
        .iter()
        .map(|dep| unresolvable_issue(dep, registries))
        .collect()
}

fn unresolvable_issue(
    dep: &UnresolvableLockfileDependency,
    registries: &HashMap<String, String>,
) -> signatures::SignatureIssue {
    let dep_path = sanitize_inline(&dep.dep_path);
    signatures::SignatureIssue {
        name: sanitize_inline(&dep.name).into_owned(),
        registry: pick_registry_for_package(registries, &dep.name, None),
        version: sanitize_inline(&dep.version).into_owned(),
        integrity: None,
        reason: Some(format!("Broken lockfile: no entry for '{dep_path}' in pnpm-lock.yaml")),
        resolved: None,
    }
}
