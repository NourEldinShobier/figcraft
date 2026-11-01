//! What a change touches: `cargo xtask ci` runs clippy and tests only for the changed workspace
//! packages and everything that depends on them. Anything it cannot place safely (a lockfile,
//! toolchain or CI change, a shallow clone, an unknown base) falls back to the full run.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

/// Paths whose change can affect every package.
const GLOBAL: &[&str] = &["Cargo.toml", "Cargo.lock", "rust-toolchain", ".cargo/", "clippy.toml", "rustfmt.toml", "deny.toml", "xtask/", ".github/"];

/// The packages the wasm check covers: a change reaching one of them runs it.
pub const WASM_PACKAGES: &[&str] = &["figcraft-collab", "figcraft-collab-net", "figcraft-engine", "figcraft-ui-egui", "figcraft-web"];

/// The workspace members: directory (relative to the root, `/`-separated), dependents and libraries.
#[derive(Default)]
pub struct Members {
    dirs: BTreeMap<String, String>,
    dependents: BTreeMap<String, BTreeSet<String>>,
    pub libs: BTreeSet<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Affected {
    pub packages: BTreeSet<String>,
    /// The `plugins/` workspace changed.
    pub plugins: bool,
}

/// The affected set for the repo at `root` against `base`: None means run everything.
pub fn detect(root: &Path, base: &str) -> Option<(Affected, Members)> {
    let members = members(root)?;
    let files = changed_files(root, base)?;
    let affected = affected(&files, &members)?;
    Some((affected, members))
}

/// The base to compare with: `--changed [BASE]` (default `origin/main`), `XTASK_BASE`, or a pull
/// request's target branch on GitHub. None: no comparison asked for.
pub fn base(args: &[String]) -> Option<String> {
    if let Some(i) = args.iter().position(|a| a == "--changed") {
        return Some(args.get(i + 1).filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "origin/main".into()));
    }
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    var("XTASK_BASE").or_else(|| var("GITHUB_BASE_REF").map(|r| format!("origin/{r}")))
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(root).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Files changed since the merge base with `base`, uncommitted and untracked ones included.
fn changed_files(root: &Path, base: &str) -> Option<Vec<String>> {
    let mb = git(root, &["merge-base", "HEAD", base])?;
    let mut files: Vec<String> = git(root, &["diff", "--name-only", mb.trim()])?.lines().map(str::to_string).collect();
    files.extend(git(root, &["ls-files", "--others", "--exclude-standard"])?.lines().map(str::to_string));
    Some(files)
}

fn members(root: &Path) -> Option<Members> {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["metadata", "--format-version", "1"])
        .current_dir(root)
        .output()
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let ids: BTreeSet<&str> = v["workspace_members"].as_array()?.iter().filter_map(|m| m.as_str()).collect();
    let mut m = Members::default();
    let mut names = BTreeMap::new();
    for p in v["packages"].as_array()? {
        let (Some(id), Some(name), Some(manifest)) = (p["id"].as_str(), p["name"].as_str(), p["manifest_path"].as_str()) else { continue };
        if !ids.contains(id) {
            continue;
        }
        names.insert(id.to_string(), name.to_string());
        let dir = Path::new(manifest).parent()?.strip_prefix(root).ok()?;
        m.dirs.insert(dir.to_string_lossy().replace('\\', "/"), name.to_string());
        if p["targets"].as_array()?.iter().any(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "lib"))) {
            m.libs.insert(name.to_string());
        }
    }
    for node in v["resolve"]["nodes"].as_array()? {
        let Some(from) = node["id"].as_str().and_then(|id| names.get(id)) else { continue };
        for dep in node["deps"].as_array()?.iter().filter_map(|d| d["pkg"].as_str()).filter_map(|id| names.get(id)) {
            m.dependents.entry(dep.clone()).or_default().insert(from.clone());
        }
    }
    Some(m)
}

/// The packages `files` reach, with their dependents. None: a global file changed.
fn affected(files: &[String], m: &Members) -> Option<Affected> {
    if files.iter().any(|f| GLOBAL.iter().any(|g| f == g || (g.ends_with('/') && f.starts_with(g)) || f.starts_with("rust-toolchain"))) {
        return None;
    }
    let plugins = files.iter().any(|f| f.starts_with("plugins/"));
    let mut todo: Vec<String> = files.iter().filter_map(|f| owner(f, m)).collect();
    let mut packages = BTreeSet::new();
    while let Some(p) = todo.pop() {
        if packages.insert(p.clone()) {
            todo.extend(m.dependents.get(&p).into_iter().flatten().cloned());
        }
    }
    Some(Affected { packages, plugins })
}

/// The package whose directory is the longest prefix of `file`.
fn owner(file: &str, m: &Members) -> Option<String> {
    m.dirs
        .iter()
        .filter(|(dir, _)| file.strip_prefix(dir.as_str()).is_some_and(|rest| rest.starts_with('/')))
        .max_by_key(|(dir, _)| dir.len())
        .map(|(_, n)| n.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members() -> Members {
        let mut m = Members::default();
        for (dir, name) in [("crates/doc", "figcraft-doc"), ("crates/engine", "figcraft-engine"), ("apps/figcraft", "figcraft")] {
            m.dirs.insert(dir.into(), name.into());
        }
        m.dependents.insert("figcraft-doc".into(), ["figcraft-engine".to_string()].into());
        m.dependents.insert("figcraft-engine".into(), ["figcraft".to_string()].into());
        m
    }

    fn names(a: &Affected) -> Vec<&str> {
        a.packages.iter().map(String::as_str).collect()
    }

    #[test]
    fn a_change_reaches_the_packages_that_depend_on_it() {
        let a = affected(&["crates/doc/src/node.rs".into()], &members()).unwrap();
        assert_eq!(names(&a), ["figcraft", "figcraft-doc", "figcraft-engine"]);
        let a = affected(&["crates/engine/src/lib.rs".into()], &members()).unwrap();
        assert_eq!(names(&a), ["figcraft", "figcraft-engine"]);
    }

    #[test]
    fn files_outside_every_package_touch_none() {
        let a = affected(&["docs/mcp.md".into(), "ROADMAP.md".into(), "plugins/examples/counter/src/lib.rs".into()], &members()).unwrap();
        assert!(a.packages.is_empty() && a.plugins);
    }

    #[test]
    fn a_lockfile_workflow_or_xtask_change_runs_everything() {
        for f in ["Cargo.lock", "Cargo.toml", ".github/workflows/ci.yml", "xtask/src/main.rs", ".cargo/config.toml", "rust-toolchain.toml"] {
            assert_eq!(affected(&[f.into()], &members()), None, "{f}");
        }
    }

    #[test]
    fn a_directory_name_that_only_shares_a_prefix_is_not_the_package() {
        assert!(affected(&["crates/doc-extra/src/lib.rs".into()], &members()).unwrap().packages.is_empty());
    }
}
