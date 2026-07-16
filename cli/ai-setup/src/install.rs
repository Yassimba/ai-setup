use crate::{Resource, ResourceKind, System};
use anyhow::Result;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Unix,
    Windows,
}

/// The oldest Node.js Pi supports (its npm package declares
/// `engines.node >= 20.6.0`). Bump when Pi does.
pub const PI_MIN_NODE: (u32, u32, u32) = (20, 6, 0);

/// What `node --version` said, reduced to the decision the planner needs.
/// ai-setup never installs or updates Node itself — it detects and instructs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeStatus {
    Missing,
    TooOld(u32, u32, u32),
    Supported,
}

impl NodeStatus {
    pub fn detect(system: &dyn System) -> Self {
        match system.run(&CommandSpec::new("node", ["--version"])) {
            Ok(result) if result.success => match parse_node_version(result.stdout.trim()) {
                Some(version) if version < PI_MIN_NODE => {
                    Self::TooOld(version.0, version.1, version.2)
                }
                // An unparseable version is not evidence of a problem; let
                // npm be the judge rather than blocking the plan.
                _ => Self::Supported,
            },
            _ => Self::Missing,
        }
    }

    /// The warning to surface, if any — `None` means Node needs nothing.
    pub fn warning(self) -> Option<String> {
        let (major, minor, patch) = PI_MIN_NODE;
        match self {
            Self::Supported => None,
            Self::Missing => Some(format!(
                "Node.js is not on PATH; Pi needs {major}.{minor}.{patch} or newer — \
                 install the current LTS from https://nodejs.org or your package manager"
            )),
            Self::TooOld(found_major, found_minor, found_patch) => Some(format!(
                "Node.js {found_major}.{found_minor}.{found_patch} is older than the \
                 {major}.{minor}.{patch} Pi needs — update it with your package manager"
            )),
        }
    }
}

/// Parse `v20.6.0` (or `20.6.0`) into a comparable triple.
fn parse_node_version(raw: &str) -> Option<(u32, u32, u32)> {
    let mut parts = raw.trim_start_matches('v').split('.');
    let mut next = || parts.next()?.parse::<u32>().ok();
    Some((next()?, next()?, next().unwrap_or(0)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrerequisiteStatus {
    pub pi: bool,
    pub herdr: bool,
    pub npm: bool,
    pub node: NodeStatus,
}

/// A manager the user can also install on its own, without selecting any
/// resource that depends on it. Skills need no runtime: the CLI copies them
/// into the agent trees itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Runtime {
    Pi,
    Herdr,
}

impl Runtime {
    pub fn installed(self, status: PrerequisiteStatus) -> bool {
        match self {
            Self::Pi => status.pi,
            Self::Herdr => status.herdr,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    pub fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerificationSpec {
    Command {
        command: CommandSpec,
        needle: Option<String>,
    },
}

/// What a plan step does when executed: run a manager command, or copy
/// skills into the detected agent trees natively.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepAction {
    Command(CommandSpec),
    CopySkills { skills: Vec<String> },
}

impl StepAction {
    pub fn display(&self) -> String {
        match self {
            Self::Command(command) => command.display(),
            Self::CopySkills { skills } => format!(
                "copy skills into detected agent trees: {}",
                skills.join(", ")
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallStep {
    pub target: String,
    pub manager: String,
    pub action: StepAction,
    pub verification: Option<VerificationSpec>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InstallPlan {
    pub prerequisites: Vec<InstallStep>,
    pub resources: Vec<InstallStep>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallFailure {
    pub target: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InstallReport {
    pub installed: Vec<String>,
    pub failures: Vec<InstallFailure>,
}

pub fn build_install_plan(
    resources: &[Resource],
    runtimes: &[Runtime],
    status: PrerequisiteStatus,
    platform: Platform,
) -> Result<InstallPlan> {
    let needs_pi = runtimes.contains(&Runtime::Pi)
        || resources
            .iter()
            .any(|resource| resource.kind == ResourceKind::PiPackage);
    let needs_herdr = runtimes.contains(&Runtime::Herdr)
        || resources
            .iter()
            .any(|resource| resource.kind == ResourceKind::HerdrPlugin);

    anyhow::ensure!(
        !needs_pi || status.pi || status.npm,
        "installing Pi needs npm, which is not on PATH; install Node.js first"
    );
    // Same preflight for the Node runtime itself: a too-old Node would make
    // `npm install` fail with an opaque engines error mid-plan.
    if needs_pi && !status.pi {
        if let Some(warning) = status.node.warning() {
            anyhow::bail!("installing Pi is blocked: {warning}");
        }
    }

    let mut prerequisites = Vec::new();
    if needs_pi && !status.pi {
        prerequisites.push(InstallStep {
            target: "Pi".into(),
            manager: "pi".into(),
            action: StepAction::Command(CommandSpec::new(
                "npm",
                ["install", "--global", "@mariozechner/pi-coding-agent"],
            )),
            verification: None,
        });
    }
    if needs_herdr && !status.herdr {
        prerequisites.push(prerequisite_step(
            "herdr",
            platform,
            "curl -fsSL https://herdr.dev/install.sh | sh",
            "irm https://herdr.dev/install.ps1 | iex",
        ));
    }

    let skills = resources
        .iter()
        .filter(|resource| resource.kind == ResourceKind::Skill)
        .map(|skill| skill.install_target.clone())
        .collect::<Vec<_>>();
    let mut steps = Vec::new();
    if !skills.is_empty() {
        steps.push(InstallStep {
            target: "skills".into(),
            manager: "skills".into(),
            action: StepAction::CopySkills { skills },
            // Verified inside the copy itself: each tree must end up with
            // <skill>/SKILL.md.
            verification: None,
        });
    }
    for resource in resources {
        let (manager, command, verification) = match resource.kind {
            ResourceKind::Skill => continue,
            ResourceKind::PiPackage => (
                "pi",
                CommandSpec::new(
                    "pi",
                    ["install", &format!("npm:{}", resource.install_target)],
                ),
                VerificationSpec::Command {
                    command: CommandSpec::new("pi", ["list"]),
                    needle: Some(resource.install_target.clone()),
                },
            ),
            ResourceKind::HerdrPlugin => (
                "herdr",
                CommandSpec::new(
                    "herdr",
                    [
                        "plugin",
                        "install",
                        resource.install_target.as_str(),
                        "--yes",
                    ],
                ),
                VerificationSpec::Command {
                    command: CommandSpec::new("herdr", ["plugin", "list"]),
                    needle: Some(resource.id.trim_start_matches("herdr-plugin:").into()),
                },
            ),
        };
        steps.push(InstallStep {
            target: resource.id.clone(),
            manager: manager.into(),
            action: StepAction::Command(command),
            verification: Some(verification),
        });
    }
    Ok(InstallPlan {
        prerequisites,
        resources: steps,
    })
}

/// Progress of one plan step, indexed over prerequisites then resources.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepStatus {
    Running,
    Prepared,
    Installed,
    Failed(String),
    Skipped(String),
}

pub fn execute_install_plan(plan: &InstallPlan, system: &(dyn System + Sync)) -> InstallReport {
    execute_install_plan_with(plan, system, &mut |_, _| {})
}

pub fn execute_install_plan_with(
    plan: &InstallPlan,
    system: &(dyn System + Sync),
    observer: &mut dyn FnMut(usize, StepStatus),
) -> InstallReport {
    let mut report = InstallReport::default();
    let mut unavailable = HashSet::new();

    // Prerequisites stay sequential: each one mutates the shared PATH and
    // the next bootstrap may depend on it (npm before pi).
    for (index, step) in plan.prerequisites.iter().enumerate() {
        observer(index, StepStatus::Running);
        let failure = execute_step(step, system).or_else(|| {
            system.refresh_path();
            (!system.command_exists(&step.manager)).then(|| {
                format!(
                    "installer completed, but {} is still unavailable on PATH",
                    step.manager
                )
            })
        });
        match failure {
            Some(message) => {
                unavailable.insert(step.manager.clone());
                report.failures.push(InstallFailure {
                    target: step.target.clone(),
                    message: message.clone(),
                });
                observer(index, StepStatus::Failed(message));
            }
            None => observer(index, StepStatus::Prepared),
        }
    }

    // Resource steps run one thread per manager: skills, pi, and herdr are
    // independent of each other, but steps sharing a manager stay in order
    // (concurrent `herdr plugin install`s would race its registry).
    let mut outcomes: Vec<Option<Option<String>>> = vec![None; plan.resources.len()];
    let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
    for (offset, step) in plan.resources.iter().enumerate() {
        if unavailable.contains(&step.manager) {
            let message = format!("{} is unavailable", step.manager.to_uppercase());
            observer(
                plan.prerequisites.len() + offset,
                StepStatus::Skipped(message.clone()),
            );
            outcomes[offset] = Some(Some(message));
            continue;
        }
        match groups
            .iter_mut()
            .find(|(manager, _)| *manager == step.manager)
        {
            Some((_, offsets)) => offsets.push(offset),
            None => groups.push((&step.manager, vec![offset])),
        }
    }

    let (event_sender, events) = std::sync::mpsc::channel::<(usize, StepStatus)>();
    std::thread::scope(|scope| {
        for (_, offsets) in &groups {
            let event_sender = event_sender.clone();
            scope.spawn(move || {
                for &offset in offsets {
                    let step = &plan.resources[offset];
                    let _ = event_sender.send((offset, StepStatus::Running));
                    let status = match execute_step(step, system) {
                        Some(message) => StepStatus::Failed(message),
                        None => StepStatus::Installed,
                    };
                    let _ = event_sender.send((offset, status));
                }
            });
        }
        drop(event_sender);
        // The observer stays on this thread (it is not Sync); workers only
        // send events.
        for (offset, status) in events {
            if !matches!(status, StepStatus::Running) {
                outcomes[offset] = Some(match &status {
                    StepStatus::Failed(message) => Some(message.clone()),
                    _ => None,
                });
            }
            observer(plan.prerequisites.len() + offset, status);
        }
    });

    // Fold outcomes in plan order so reports stay deterministic regardless
    // of which manager finished first.
    for (step, outcome) in plan.resources.iter().zip(outcomes) {
        match outcome {
            Some(Some(message)) => report.failures.push(InstallFailure {
                target: step.target.clone(),
                message,
            }),
            Some(None) => report.installed.push(step.target.clone()),
            None => {}
        }
    }
    report
}

fn execute_step(step: &InstallStep, system: &dyn System) -> Option<String> {
    let command = match &step.action {
        StepAction::CopySkills { skills } => {
            return crate::skills::install_skills(system, skills).err()
        }
        StepAction::Command(command) => command,
    };
    match system.run(command) {
        Ok(result) if result.success => {}
        Ok(result) => return Some(command_failure_message(&result)),
        Err(error) => return Some(error.to_string()),
    }
    let Some(verification) = &step.verification else {
        return None;
    };
    match verification {
        VerificationSpec::Command { command, needle } => match system.run(command) {
            Ok(result) if !result.success => Some(format!(
                "verification failed: {}",
                command_failure_message(&result)
            )),
            Ok(result) => {
                let output = format!("{}\n{}", result.stdout, result.stderr);
                needle
                    .as_ref()
                    .filter(|needle| !output.contains(needle.as_str()))
                    .map(|needle| format!("verification did not find {needle}"))
            }
            Err(error) => Some(format!("verification failed: {error}")),
        },
    }
}

/// The interesting half of a failed command's output: stderr, else stdout.
pub(crate) fn command_failure_message(result: &crate::CommandResult) -> String {
    if result.stderr.trim().is_empty() {
        result.stdout.trim().to_owned()
    } else {
        result.stderr.trim().to_owned()
    }
}

fn prerequisite_step(
    manager: &str,
    platform: Platform,
    unix_script: &str,
    windows_script: &str,
) -> InstallStep {
    let command = match platform {
        Platform::Unix => CommandSpec::new("sh", ["-c", unix_script]),
        Platform::Windows => CommandSpec::new(
            "powershell",
            [
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                windows_script,
            ],
        ),
    };
    InstallStep {
        target: manager.to_uppercase(),
        manager: manager.into(),
        action: StepAction::Command(command),
        verification: None,
    }
}
