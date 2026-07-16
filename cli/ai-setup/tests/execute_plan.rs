use ai_setup::{
    execute_install_plan, CommandResult, CommandSpec, InstallPlan, InstallStep, StepAction, System,
};
use std::fs;
use std::path::{Path, PathBuf};

struct FakeSystem {
    commands: std::sync::Mutex<Vec<String>>,
}

impl System for FakeSystem {
    fn command_exists(&self, _name: &str) -> bool {
        false
    }

    fn refresh_path(&self) {}

    fn run(&self, command: &CommandSpec) -> anyhow::Result<CommandResult> {
        let display = command.display();
        self.commands.lock().unwrap().push(display.clone());
        if display.contains("herdr.dev/install") {
            Ok(CommandResult {
                success: false,
                stdout: String::new(),
                stderr: "network unavailable".into(),
            })
        } else {
            Ok(CommandResult {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }
}

fn step(target: &str, manager: &str, program: &str) -> InstallStep {
    InstallStep {
        target: target.into(),
        manager: manager.into(),
        action: StepAction::Command(CommandSpec::new(program, std::iter::empty::<String>())),
        verification: None,
    }
}

#[test]
fn failed_prerequisite_skips_only_resources_that_need_that_manager() {
    let plan = InstallPlan {
        prerequisites: vec![InstallStep {
            target: "HERDR".into(),
            manager: "herdr".into(),
            action: StepAction::Command(CommandSpec::new(
                "sh",
                ["-c", "curl -fsSL https://herdr.dev/install.sh | sh"],
            )),
            verification: None,
        }],
        resources: vec![
            step("herdr-plugin:jumplist", "herdr", "herdr"),
            step("pi-package:sample", "pi", "pi"),
        ],
    };
    let system = FakeSystem {
        commands: std::sync::Mutex::new(Vec::new()),
    };

    let report = execute_install_plan(&plan, &system);

    assert_eq!(report.installed, vec!["pi-package:sample"]);
    assert_eq!(report.failures.len(), 2);
    assert_eq!(report.failures[0].target, "HERDR");
    assert_eq!(report.failures[0].message, "network unavailable");
    assert_eq!(report.failures[1].target, "herdr-plugin:jumplist");
    assert_eq!(report.failures[1].message, "HERDR is unavailable");
    assert_eq!(
        system.commands.into_inner().unwrap(),
        vec!["sh -c curl -fsSL https://herdr.dev/install.sh | sh", "pi",]
    );
}

struct HiddenCommandSystem;

impl System for HiddenCommandSystem {
    fn command_exists(&self, _name: &str) -> bool {
        false
    }

    fn refresh_path(&self) {}

    fn run(&self, _command: &CommandSpec) -> anyhow::Result<CommandResult> {
        Ok(CommandResult {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

#[test]
fn successful_bootstrap_must_make_its_manager_available() {
    let plan = InstallPlan {
        prerequisites: vec![InstallStep {
            target: "HERDR".into(),
            manager: "herdr".into(),
            action: StepAction::Command(CommandSpec::new("sh", ["-c", "install herdr"])),
            verification: None,
        }],
        resources: vec![step("herdr-plugin:jumplist", "herdr", "herdr")],
    };

    let report = execute_install_plan(&plan, &HiddenCommandSystem);

    assert!(report.installed.is_empty());
    assert_eq!(report.failures[0].target, "HERDR");
    assert_eq!(
        report.failures[0].message,
        "installer completed, but herdr is still unavailable on PATH"
    );
    assert_eq!(report.failures[1].message, "HERDR is unavailable");
}

/// Fakes curl and tar with filesystem side effects, so the native skill
/// installer runs end to end against a temp home.
struct SkillInstallSystem {
    home: PathBuf,
    /// The skills the "downloaded" repo contains.
    repo_skills: Vec<String>,
}

impl SkillInstallSystem {
    fn new(label: &str, repo_skills: &[&str]) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let home = std::env::temp_dir().join(format!(
            "ai-setup-exec-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(home.join(".claude")).expect("create temp home");
        Self {
            home,
            repo_skills: repo_skills.iter().map(ToString::to_string).collect(),
        }
    }

    fn staging(&self) -> PathBuf {
        self.home.join(".cache").join("ai-setup").join("staging")
    }

    fn tree(&self) -> PathBuf {
        self.home.join(".claude").join("skills")
    }
}

impl Drop for SkillInstallSystem {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.home).ok();
    }
}

impl System for SkillInstallSystem {
    fn command_exists(&self, _name: &str) -> bool {
        true
    }

    fn refresh_path(&self) {}

    fn home_dir(&self) -> Option<PathBuf> {
        Some(self.home.clone())
    }

    fn run(&self, command: &CommandSpec) -> anyhow::Result<CommandResult> {
        match command.program.as_str() {
            "curl" => {
                let output = command
                    .args
                    .iter()
                    .skip_while(|arg| arg.as_str() != "-o")
                    .nth(1)
                    .expect("curl is invoked with -o <path>");
                fs::write(Path::new(output), b"tarball")?;
            }
            "tar" => {
                for name in &self.repo_skills {
                    let skill = self
                        .staging()
                        .join("ai-setup-main")
                        .join("skills")
                        .join(name);
                    fs::create_dir_all(&skill)?;
                    fs::write(skill.join("SKILL.md"), format!("# {name}\n"))?;
                }
            }
            other => panic!("unexpected command: {other}"),
        }
        Ok(CommandResult {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn copy_skills_plan(skills: &[&str]) -> InstallPlan {
    InstallPlan {
        prerequisites: Vec::new(),
        resources: vec![InstallStep {
            target: "skills".into(),
            manager: "skills".into(),
            action: StepAction::CopySkills {
                skills: skills.iter().map(ToString::to_string).collect(),
            },
            verification: None,
        }],
    }
}

#[test]
fn skills_are_copied_into_every_detected_tree() {
    let system = SkillInstallSystem::new("copy", &["tdd", "commit"]);

    let report = execute_install_plan(&copy_skills_plan(&["tdd", "commit"]), &system);

    assert_eq!(report.installed, vec!["skills"]);
    assert!(report.failures.is_empty());
    assert!(system.tree().join("tdd").join("SKILL.md").is_file());
    assert!(system.tree().join("commit").join("SKILL.md").is_file());
    assert!(!system.staging().exists(), "staging should be cleaned up");
}

#[test]
fn a_skill_missing_from_the_downloaded_repo_fails_the_step() {
    let system = SkillInstallSystem::new("missing", &["commit"]);

    let report = execute_install_plan(&copy_skills_plan(&["tdd"]), &system);

    assert!(report.installed.is_empty());
    assert_eq!(report.failures[0].target, "skills");
    assert_eq!(
        report.failures[0].message,
        "downloaded repo is missing skills: tdd"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_skills_survive_an_install_untouched() {
    let system = SkillInstallSystem::new("symlink", &["tdd"]);
    let checkout = system.home.join("checkout").join("tdd");
    fs::create_dir_all(&checkout).unwrap();
    fs::write(checkout.join("SKILL.md"), "# local checkout\n").unwrap();
    fs::create_dir_all(system.tree()).unwrap();
    std::os::unix::fs::symlink(&checkout, system.tree().join("tdd")).unwrap();

    let report = execute_install_plan(&copy_skills_plan(&["tdd"]), &system);

    assert_eq!(report.installed, vec!["skills"]);
    let target = system.tree().join("tdd");
    assert!(target.symlink_metadata().unwrap().is_symlink());
    assert_eq!(
        fs::read_to_string(target.join("SKILL.md")).unwrap(),
        "# local checkout\n"
    );
}

#[test]
fn parallel_managers_still_report_in_plan_order() {
    let plan = InstallPlan {
        prerequisites: Vec::new(),
        resources: vec![
            step("pi-package:one", "pi", "pi"),
            step("herdr-plugin:two", "herdr", "herdr"),
            step("pi-package:three", "pi", "pi"),
        ],
    };

    let report = execute_install_plan(&plan, &HiddenCommandSystem);

    assert!(report.failures.is_empty());
    assert_eq!(
        report.installed,
        vec!["pi-package:one", "herdr-plugin:two", "pi-package:three"],
        "report order must follow the plan, not thread completion"
    );
}
