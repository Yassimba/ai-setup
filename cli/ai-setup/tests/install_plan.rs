use ai_setup::{
    build_install_plan, expand_skill_dependencies, CommandSpec, NodeStatus, Platform,
    PrerequisiteStatus, Resource, ResourceKind, Runtime, StepAction, VerificationSpec,
};
use pretty_assertions::assert_eq;

fn resource(kind: ResourceKind, id: &str, target: &str) -> Resource {
    Resource {
        id: id.into(),
        kind,
        group: "Test".into(),
        label: id.into(),
        description: "Test resource".into(),
        install_target: target.into(),
        next_action: "Try it".into(),
        dependencies: Vec::new(),
    }
}

fn skill_with_deps(id: &str, target: &str, dependencies: &[&str]) -> Resource {
    Resource {
        dependencies: dependencies.iter().map(ToString::to_string).collect(),
        ..resource(ResourceKind::Skill, id, target)
    }
}

#[test]
fn mixed_selection_copies_skills_and_delegates_the_rest() {
    let resources = vec![
        resource(ResourceKind::Skill, "skill:tdd", "tdd"),
        resource(
            ResourceKind::PiPackage,
            "pi-package:@yassimba/pi-openai-fast",
            "@yassimba/pi-openai-fast",
        ),
        resource(
            ResourceKind::HerdrPlugin,
            "herdr-plugin:yassin.jumplist",
            "Yassimba/ai-setup/plugins/herdr-jumplist",
        ),
    ];
    let status = PrerequisiteStatus {
        pi: true,
        herdr: true,
        npm: true,
        node: NodeStatus::Supported,
    };

    let plan = build_install_plan(&resources, &[], status, Platform::Unix).unwrap();

    assert!(plan.prerequisites.is_empty());
    assert_eq!(
        plan.resources
            .iter()
            .map(|step| step.action.clone())
            .collect::<Vec<_>>(),
        vec![
            StepAction::CopySkills {
                skills: vec!["tdd".into()],
            },
            StepAction::Command(CommandSpec::new(
                "pi",
                ["install", "npm:@yassimba/pi-openai-fast"],
            )),
            StepAction::Command(CommandSpec::new(
                "herdr",
                [
                    "plugin",
                    "install",
                    "Yassimba/ai-setup/plugins/herdr-jumplist",
                    "--yes",
                ],
            )),
        ]
    );
    assert_eq!(
        plan.resources
            .iter()
            .map(|step| step.verification.clone())
            .collect::<Vec<_>>(),
        vec![
            // Skills are verified inside the copy: each tree must end up
            // with <skill>/SKILL.md.
            None,
            Some(VerificationSpec::Command {
                command: CommandSpec::new("pi", ["list"]),
                needle: Some("@yassimba/pi-openai-fast".into()),
            }),
            Some(VerificationSpec::Command {
                command: CommandSpec::new("herdr", ["plugin", "list"]),
                needle: Some("yassin.jumplist".into()),
            }),
        ]
    );
}

#[test]
fn skill_selection_expands_to_its_dependency_closure() {
    let catalog = vec![
        skill_with_deps("skill:release", "release", &["commit"]),
        skill_with_deps("skill:commit", "commit", &["writing-clearly-and-concisely"]),
        skill_with_deps(
            "skill:writing-clearly-and-concisely",
            "writing-clearly-and-concisely",
            &[],
        ),
        skill_with_deps("skill:unrelated", "unrelated", &[]),
    ];

    let expanded = expand_skill_dependencies(&catalog, vec![catalog[0].clone()]);
    let plan = build_install_plan(
        &expanded,
        &[],
        PrerequisiteStatus {
            pi: true,
            herdr: true,
            npm: true,
            node: NodeStatus::Supported,
        },
        Platform::Unix,
    )
    .unwrap();

    assert_eq!(
        plan.resources[0].action,
        StepAction::CopySkills {
            skills: vec![
                "release".into(),
                "commit".into(),
                "writing-clearly-and-concisely".into(),
            ],
        }
    );
}

#[test]
fn missing_foundations_are_installed_before_selected_resources() {
    let resources = vec![
        resource(ResourceKind::Skill, "skill:tdd", "tdd"),
        resource(
            ResourceKind::HerdrPlugin,
            "herdr-plugin:yassin.jumplist",
            "Yassimba/ai-setup/plugins/herdr-jumplist",
        ),
    ];
    let status = PrerequisiteStatus {
        pi: true,
        herdr: false,
        npm: true,
        node: NodeStatus::Supported,
    };

    let plan = build_install_plan(&resources, &[], status, Platform::Windows).unwrap();

    // Skills need no prerequisite manager — only Herdr goes in first.
    assert_eq!(
        plan.prerequisites
            .iter()
            .map(|step| step.action.display())
            .collect::<Vec<_>>(),
        vec![
            "powershell -NoProfile -ExecutionPolicy Bypass -Command irm https://herdr.dev/install.ps1 | iex",
        ]
    );
}

#[test]
fn selecting_a_pi_package_without_pi_or_npm_gives_an_actionable_error() {
    let resources = vec![resource(
        ResourceKind::PiPackage,
        "pi-package:@yassimba/pi-openai-fast",
        "@yassimba/pi-openai-fast",
    )];
    let status = PrerequisiteStatus {
        pi: false,
        herdr: true,
        npm: false,
        node: NodeStatus::Supported,
    };

    let error = build_install_plan(&resources, &[], status, Platform::Unix).unwrap_err();

    assert_eq!(
        error.to_string(),
        "installing Pi needs npm, which is not on PATH; install Node.js first"
    );
}

#[test]
fn explicitly_requested_runtimes_are_installed_without_dependent_resources() {
    let status = PrerequisiteStatus {
        pi: false,
        herdr: false,
        npm: true,
        node: NodeStatus::Supported,
    };

    let plan =
        build_install_plan(&[], &[Runtime::Herdr, Runtime::Pi], status, Platform::Unix).unwrap();

    assert!(plan.resources.is_empty());
    assert_eq!(
        plan.prerequisites
            .iter()
            .map(|step| step.action.display())
            .collect::<Vec<_>>(),
        vec![
            "npm install --global @mariozechner/pi-coding-agent",
            "sh -c curl -fsSL https://herdr.dev/install.sh | sh",
        ]
    );
}

#[test]
fn an_already_installed_runtime_request_is_a_no_op() {
    let plan = build_install_plan(
        &[],
        &[Runtime::Herdr],
        PrerequisiteStatus {
            pi: true,
            herdr: true,
            npm: true,
            node: NodeStatus::Supported,
        },
        Platform::Unix,
    )
    .unwrap();

    assert!(plan.prerequisites.is_empty());
    assert!(plan.resources.is_empty());
}

#[test]
fn an_outdated_node_blocks_a_fresh_pi_install_with_instructions() {
    let resources = vec![resource(
        ResourceKind::PiPackage,
        "pi-package:@yassimba/pi-openai-fast",
        "@yassimba/pi-openai-fast",
    )];
    let status = PrerequisiteStatus {
        pi: false,
        herdr: true,
        npm: true,
        node: NodeStatus::TooOld(16, 3, 0),
    };

    let error = build_install_plan(&resources, &[], status, Platform::Unix).unwrap_err();

    assert_eq!(
        error.to_string(),
        "installing Pi is blocked: Node.js 16.3.0 is older than the 20.6.0 Pi needs — \
         update it with your package manager"
    );
}

#[test]
fn an_installed_pi_does_not_care_about_node() {
    // Pi already on PATH means npm never runs; an old Node must not block.
    let resources = vec![resource(
        ResourceKind::PiPackage,
        "pi-package:@yassimba/pi-openai-fast",
        "@yassimba/pi-openai-fast",
    )];
    let status = PrerequisiteStatus {
        pi: true,
        herdr: true,
        npm: false,
        node: NodeStatus::Missing,
    };

    let plan = build_install_plan(&resources, &[], status, Platform::Unix).unwrap();

    assert_eq!(plan.resources.len(), 1);
}
