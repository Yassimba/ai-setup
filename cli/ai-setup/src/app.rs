use crate::settings::{curated_settings, setting_state, SettingsPaths};
use crate::ui::{confirm_plan, print_plan};
use crate::wizard::{run_wizard, Model, WizardOutcome};
use crate::{
    build_install_plan, execute_install_plan, expand_skill_dependencies, Catalog, CommandSpec,
    InstallReport, NodeStatus, Platform, PrerequisiteStatus, Resource, ResourceKind, System,
};
use anyhow::{bail, Context, Result};

#[derive(Default)]
pub struct Selectors {
    pub skills: Vec<String>,
    pub pi_packages: Vec<String>,
    pub herdr_plugins: Vec<String>,
}

impl Selectors {
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty() && self.pi_packages.is_empty() && self.herdr_plugins.is_empty()
    }
}

pub fn install_selected(
    catalog: &Catalog,
    selectors: &Selectors,
    assume_yes: bool,
    dry_run: bool,
    system: &(dyn System + Sync),
) -> Result<bool> {
    let status = PrerequisiteStatus {
        pi: system.command_exists("pi"),
        herdr: system.command_exists("herdr"),
        npm: system.command_exists("npm"),
        node: NodeStatus::detect(system),
    };
    let platform = if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Unix
    };
    if selectors.is_empty() {
        return run_interactive(catalog, status, platform, dry_run, system);
    }

    let resources =
        expand_skill_dependencies(&catalog.resources, resolve_selectors(catalog, selectors)?);
    if resources.is_empty() {
        println!("Nothing selected; no changes made.");
        return Ok(true);
    }
    let plan = build_install_plan(&resources, &[], status, platform)?;
    print_plan(&plan);
    if dry_run {
        println!("\nDry run; no changes made.");
        return Ok(true);
    }
    if !assume_yes && !confirm_plan()? {
        println!("Cancelled; no changes made.");
        return Ok(true);
    }

    let report = execute_install_plan(&plan, system);
    print_report(&report);
    if let Some(next) = resources
        .iter()
        .find(|resource| {
            report.installed.contains(&resource.id)
                || (resource.kind == ResourceKind::Skill
                    && report.installed.iter().any(|target| target == "skills"))
        })
        .map(|resource| resource.next_action.as_str())
    {
        println!("\nNext: {next}");
    }
    Ok(report.failures.is_empty())
}

fn run_interactive(
    catalog: &Catalog,
    status: PrerequisiteStatus,
    platform: Platform,
    dry_run: bool,
    system: &(dyn System + Sync),
) -> Result<bool> {
    let settings_paths = SettingsPaths::detect()?;
    let settings = curated_settings();
    let setting_states = settings
        .iter()
        .map(|spec| setting_state(spec, &settings_paths))
        .collect();
    let zed_present = settings_paths.zed_settings.exists();
    let installed = detect_installed(&catalog.resources, status, system);
    let model = Model {
        resources: catalog.resources.clone(),
        installed,
        settings,
        setting_states,
        zed_present,
        settings_paths,
        status,
        platform,
        dry_run,
    };
    match run_wizard(model, system)? {
        WizardOutcome::Cancelled => {
            println!("Cancelled; no changes made.");
            Ok(true)
        }
        WizardOutcome::NothingSelected => {
            println!("Nothing selected; no changes made.");
            Ok(true)
        }
        WizardOutcome::DryRun(plan, setting_changes) => {
            print_plan(&plan);
            for change in &setting_changes {
                println!("  Configure {change}");
            }
            println!("\nDry run; no changes made.");
            Ok(true)
        }
        WizardOutcome::Installed(report) => {
            print_report(&report);
            Ok(report.failures.is_empty())
        }
    }
}

/// Which catalog resources are already on this machine. Uses the same
/// probes as post-install verification: manager list output for plugins and
/// packages, the global agent trees for skills.
fn detect_installed(
    resources: &[Resource],
    status: PrerequisiteStatus,
    system: &(dyn System + Sync),
) -> Vec<bool> {
    let list_output = |present: bool, program: &str, args: &[&str]| {
        if !present {
            return None;
        }
        system
            .run(&CommandSpec::new(program, args.iter().copied()))
            .ok()
            .filter(|result| result.success)
            .map(|result| format!("{}\n{}", result.stdout, result.stderr))
    };
    // Both list commands shell out to their manager; probe them concurrently.
    let (herdr_plugins, pi_packages) = std::thread::scope(|scope| {
        let herdr = scope.spawn(|| list_output(status.herdr, "herdr", &["plugin", "list"]));
        let pi = scope.spawn(|| list_output(status.pi, "pi", &["list"]));
        (
            herdr.join().expect("herdr probe thread"),
            pi.join().expect("pi probe thread"),
        )
    });
    let skill_trees = system
        .home_dir()
        .map(|home| crate::detect_skill_trees(&home))
        .unwrap_or_default();

    resources
        .iter()
        .map(|resource| match resource.kind {
            ResourceKind::HerdrPlugin => herdr_plugins.as_ref().is_some_and(|output| {
                output.contains(resource.id.trim_start_matches("herdr-plugin:"))
            }),
            ResourceKind::PiPackage => pi_packages.as_ref().is_some_and(|output| {
                // `pi list` prints npm specs for registry installs and
                // directory paths for local ones; accept either shape.
                let unscoped = resource
                    .install_target
                    .rsplit('/')
                    .next()
                    .unwrap_or(&resource.install_target);
                let plain = unscoped.strip_prefix("pi-").unwrap_or(unscoped);
                let last_component_is = |line: &str, name: &str| {
                    line.ends_with(&format!("/{name}")) || line.ends_with(&format!("\\{name}"))
                };
                output.lines().map(str::trim).any(|line| {
                    line.contains(&resource.install_target)
                        || last_component_is(line, unscoped)
                        || last_component_is(line, plain)
                })
            }),
            ResourceKind::Skill => skill_trees
                .iter()
                .any(|tree| crate::skills::skill_present_in(tree, &resource.install_target)),
        })
        .collect()
}

fn print_report(report: &InstallReport) {
    if !report.installed.is_empty() {
        println!("\nInstalled:");
        for target in &report.installed {
            println!("  ✓ {target}");
        }
    }
    if !report.failures.is_empty() {
        eprintln!("\nCould not install:");
        for failure in &report.failures {
            eprintln!("  ! {}: {}", failure.target, failure.message);
        }
    }
}

fn resolve_selectors(catalog: &Catalog, selectors: &Selectors) -> Result<Vec<Resource>> {
    let mut selected = Vec::new();
    for (kind, values) in [
        (ResourceKind::Skill, &selectors.skills),
        (ResourceKind::PiPackage, &selectors.pi_packages),
        (ResourceKind::HerdrPlugin, &selectors.herdr_plugins),
    ] {
        for value in values {
            let matches = catalog
                .resources
                .iter()
                .filter(|resource| {
                    resource.kind == kind
                        && (resource.id == *value
                            || resource.install_target == *value
                            || resource.label.eq_ignore_ascii_case(value)
                            || resource.id.ends_with(&format!(":{value}"))
                            || resource.install_target.ends_with(&format!("/{value}")))
                })
                .cloned()
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [resource] => {
                    if !selected.contains(resource) {
                        selected.push(resource.clone());
                    }
                }
                [] => bail!("unknown {kind}: {value}"),
                _ => bail!("ambiguous {kind}: {value}"),
            }
        }
    }
    Ok(selected)
}

pub fn load_catalog() -> Result<Catalog> {
    Catalog::embedded().context("could not load the curated setup catalog")
}
