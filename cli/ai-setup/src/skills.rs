//! Native skill installer. Skills are plain directories in this repo;
//! installing one means downloading the repo tarball once and copying
//! `skills/<name>` into every agent skill tree present on the machine.
//! No package manager sits in between.

use crate::{CommandSpec, Resource, ResourceKind, System};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub const TARBALL_URL: &str =
    "https://codeload.github.com/Yassimba/ai-setup/tar.gz/refs/heads/main";

/// Agent directories whose `skills/` child receives installs. The agent dir
/// existing is the signal that the agent itself is installed; the skills
/// subdirectory is created on demand.
const AGENT_DIRS: [&[&str]; 4] = [&[".claude"], &[".agents"], &[".codex"], &[".pi", "agent"]];

/// The supported agent directories, for user-facing messages:
/// `~/.claude, ~/.agents, ~/.codex, ~/.pi/agent`.
pub(crate) fn agent_dirs_display() -> String {
    AGENT_DIRS
        .map(|components| format!("~/{}", components.join("/")))
        .join(", ")
}

/// One definition of "installed": the skill's directory (real or symlinked)
/// holds a SKILL.md in this tree. The wizard's detection and update's
/// refresh union must agree on this.
pub(crate) fn skill_present_in(tree: &Path, name: &str) -> bool {
    tree.join(name).join("SKILL.md").is_file()
}

/// The skill trees to install into: `<agent dir>/skills` for every agent
/// directory that exists under `home`.
pub fn detect_skill_trees(home: &Path) -> Vec<PathBuf> {
    AGENT_DIRS
        .iter()
        .map(|components| {
            components
                .iter()
                .fold(home.to_path_buf(), |path, part| path.join(part))
        })
        .filter(|agent_dir| agent_dir.is_dir())
        .map(|agent_dir| agent_dir.join("skills"))
        .collect()
}

/// The selection plus the transitive closure of catalog skill dependencies,
/// deduplicated, dependencies appended after the explicit selection.
pub fn expand_skill_dependencies(all: &[Resource], selection: Vec<Resource>) -> Vec<Resource> {
    let mut expanded = selection;
    let mut seen = expanded
        .iter()
        .map(|resource| resource.id.clone())
        .collect::<HashSet<_>>();
    let mut cursor = 0;
    while cursor < expanded.len() {
        let dependencies = expanded[cursor].dependencies.clone();
        cursor += 1;
        for name in dependencies {
            let Some(dependency) = all
                .iter()
                .find(|candidate| {
                    candidate.kind == ResourceKind::Skill && candidate.install_target == name
                })
                .cloned()
            else {
                continue; // catalog generation guarantees deps resolve; stay lenient here
            };
            if seen.insert(dependency.id.clone()) {
                expanded.push(dependency);
            }
        }
    }
    expanded
}

/// Catalog skills already present (as a real directory or a symlink holding a
/// SKILL.md) in any detected tree — the set `ai-setup update` refreshes.
pub fn installed_catalog_skills(resources: &[Resource], home: &Path) -> Vec<Resource> {
    let trees = detect_skill_trees(home);
    resources
        .iter()
        .filter(|resource| resource.kind == ResourceKind::Skill)
        .filter(|resource| {
            trees
                .iter()
                .any(|tree| skill_present_in(tree, &resource.install_target))
        })
        .cloned()
        .collect()
}

/// What happened to one skill in one tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CopyOutcome {
    Installed,
    /// The tree entry is a symlink (e.g. a dev machine linking into a repo
    /// checkout); never write through it.
    SkippedSymlink,
}

/// Per-tree summary of an install run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeReport {
    pub tree: PathBuf,
    pub installed: usize,
    pub skipped_symlinks: Vec<String>,
}

/// Download the repo once and copy every named skill into every detected
/// skill tree, then verify each copy landed with its SKILL.md. Returns the
/// per-tree summaries, or one aggregated failure message.
pub fn install_skills(system: &dyn System, skills: &[String]) -> Result<Vec<TreeReport>, String> {
    let home = system
        .home_dir()
        .ok_or_else(|| "home directory is unavailable".to_string())?;
    let trees = detect_skill_trees(&home);
    if trees.is_empty() {
        return Err(format!(
            "no supported agent directory found ({})",
            agent_dirs_display()
        ));
    }

    let staging = home.join(".cache").join("ai-setup").join("staging");
    let result = fetch_repo(system, &staging).and_then(|repo_root| {
        let source_root = repo_root.join("skills");
        let mut missing = skills
            .iter()
            .filter(|name| !source_root.join(name.as_str()).join("SKILL.md").is_file())
            .peekable();
        if missing.peek().is_some() {
            return Err(format!(
                "downloaded repo is missing skills: {}",
                missing.cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        copy_into_trees(&source_root, &trees, skills)
    });
    let _ = fs::remove_dir_all(&staging);
    result
}

/// Download and unpack the repo tarball into `staging`; returns the extracted
/// repo root. Shells out to curl and tar (present on macOS, Linux, and
/// Windows 10+) through `System`, so tests can intercept both.
fn fetch_repo(system: &dyn System, staging: &Path) -> Result<PathBuf, String> {
    let _ = fs::remove_dir_all(staging);
    fs::create_dir_all(staging)
        .map_err(|error| format!("could not create {}: {error}", staging.display()))?;
    let tarball = staging.join("repo.tar.gz");
    for spec in [
        CommandSpec::new(
            "curl",
            ["-fsSL", TARBALL_URL, "-o", &tarball.display().to_string()],
        ),
        CommandSpec::new(
            "tar",
            [
                "-xzf",
                &tarball.display().to_string(),
                "-C",
                &staging.display().to_string(),
            ],
        ),
    ] {
        match system.run(&spec) {
            Ok(result) if result.success => {}
            Ok(result) => {
                return Err(format!(
                    "{} failed: {}",
                    spec.program,
                    crate::install::command_failure_message(&result)
                ));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    // The tarball unpacks to a single top-level directory whose name embeds
    // the ref; locate it rather than hardcoding "ai-setup-main".
    let entries = fs::read_dir(staging)
        .map_err(|error| format!("could not read {}: {error}", staging.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    match entries.as_slice() {
        [root] => Ok(root.clone()),
        _ => Err("downloaded tarball did not unpack to a single directory".to_string()),
    }
}

fn copy_into_trees(
    source_root: &Path,
    trees: &[PathBuf],
    skills: &[String],
) -> Result<Vec<TreeReport>, String> {
    let mut reports = Vec::new();
    let mut failures = Vec::new();
    for tree in trees {
        if let Err(error) = fs::create_dir_all(tree) {
            failures.push(format!("could not create {}: {error}", tree.display()));
            continue;
        }
        let mut report = TreeReport {
            tree: tree.clone(),
            installed: 0,
            skipped_symlinks: Vec::new(),
        };
        for name in skills {
            match copy_skill(source_root, tree, name) {
                Ok(CopyOutcome::Installed) => {
                    if tree.join(name).join("SKILL.md").is_file() {
                        report.installed += 1;
                    } else {
                        failures.push(format!(
                            "{} landed without a SKILL.md in {}",
                            name,
                            tree.display()
                        ));
                    }
                }
                Ok(CopyOutcome::SkippedSymlink) => report.skipped_symlinks.push(name.clone()),
                Err(error) => failures.push(format!("{name} into {}: {error}", tree.display())),
            }
        }
        reports.push(report);
    }
    if failures.is_empty() {
        Ok(reports)
    } else {
        Err(failures.join("; "))
    }
}

/// Copy one skill into one tree. Replacement goes through a temp sibling and
/// a rename, so a half-written skill never sits at the final path.
fn copy_skill(source_root: &Path, tree: &Path, name: &str) -> Result<CopyOutcome, String> {
    let target = tree.join(name);
    if target
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Ok(CopyOutcome::SkippedSymlink);
    }
    let incoming = tree.join(format!(".{name}.ai-setup-new"));
    let _ = fs::remove_dir_all(&incoming);
    copy_dir(&source_root.join(name), &incoming).map_err(|error| error.to_string())?;
    if target.exists() {
        fs::remove_dir_all(&target).map_err(|error| error.to_string())?;
    }
    fs::rename(&incoming, &target).map_err(|error| error.to_string())?;
    Ok(CopyOutcome::Installed)
}

fn copy_dir(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_str() == Some(".DS_Store") {
            continue;
        }
        let from = entry.path();
        let to = destination.join(&name);
        if entry.file_type()?.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResourceKind;

    fn skill(name: &str, dependencies: &[&str]) -> Resource {
        Resource {
            id: format!("skill:{name}"),
            kind: ResourceKind::Skill,
            group: "Coding".into(),
            label: name.into(),
            description: String::new(),
            install_target: name.into(),
            next_action: String::new(),
            dependencies: dependencies.iter().map(ToString::to_string).collect(),
        }
    }

    fn temp_home(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let home = std::env::temp_dir().join(format!(
            "ai-setup-skills-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&home).expect("create temp home");
        home
    }

    fn write_skill(root: &Path, name: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).expect("create skill dir");
        fs::write(dir.join("SKILL.md"), format!("# {name}\n")).expect("write SKILL.md");
    }

    #[test]
    fn trees_are_detected_only_for_existing_agent_dirs() {
        let home = temp_home("detect");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(home.join(".pi").join("agent")).unwrap();

        let trees = detect_skill_trees(&home);

        assert_eq!(
            trees,
            vec![
                home.join(".claude").join("skills"),
                home.join(".pi").join("agent").join("skills"),
            ]
        );
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn dependency_closure_is_transitive_and_cycle_tolerant() {
        let all = vec![
            skill("release", &["commit"]),
            skill("commit", &["writing"]),
            skill("writing", &["release"]), // cycle back to the root
            skill("unrelated", &[]),
        ];

        let expanded = expand_skill_dependencies(&all, vec![all[0].clone()]);

        assert_eq!(
            expanded
                .iter()
                .map(|resource| resource.label.as_str())
                .collect::<Vec<_>>(),
            vec!["release", "commit", "writing"]
        );
    }

    #[test]
    fn copying_replaces_stale_content_via_temp_sibling() {
        let home = temp_home("replace");
        let source = home.join("source");
        write_skill(&source, "tdd");
        let tree = home.join("tree");
        fs::create_dir_all(tree.join("tdd")).unwrap();
        fs::write(tree.join("tdd").join("stale.md"), "old").unwrap();

        let outcome = copy_skill(&source, &tree, "tdd").expect("copy");

        assert_eq!(outcome, CopyOutcome::Installed);
        assert!(tree.join("tdd").join("SKILL.md").is_file());
        assert!(!tree.join("tdd").join("stale.md").exists());
        assert!(!tree.join(".tdd.ai-setup-new").exists());
        fs::remove_dir_all(&home).ok();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_skills_are_never_written_through() {
        let home = temp_home("symlink");
        let source = home.join("source");
        write_skill(&source, "tdd");
        let checkout = home.join("checkout");
        write_skill(&checkout, "tdd");
        let tree = home.join("tree");
        fs::create_dir_all(&tree).unwrap();
        std::os::unix::fs::symlink(checkout.join("tdd"), tree.join("tdd")).unwrap();

        let outcome = copy_skill(&source, &tree, "tdd").expect("copy");

        assert_eq!(outcome, CopyOutcome::SkippedSymlink);
        assert!(tree.join("tdd").symlink_metadata().unwrap().is_symlink());
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn installed_skills_are_the_union_across_trees() {
        let home = temp_home("union");
        let claude = home.join(".claude").join("skills");
        let codex = home.join(".codex").join("skills");
        write_skill(&claude, "tdd");
        write_skill(&codex, "commit");

        let all = vec![skill("tdd", &[]), skill("commit", &[]), skill("other", &[])];
        let installed = installed_catalog_skills(&all, &home);

        assert_eq!(
            installed
                .iter()
                .map(|resource| resource.label.as_str())
                .collect::<Vec<_>>(),
            vec!["tdd", "commit"]
        );
        fs::remove_dir_all(&home).ok();
    }
}
