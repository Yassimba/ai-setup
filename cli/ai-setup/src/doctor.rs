use crate::{skills, CommandSpec, System};

pub struct Check {
    pub name: &'static str,
    pub installed: bool,
    pub healthy: bool,
    pub detail: String,
}

pub fn run_doctor(system: &(dyn System + Sync)) -> bool {
    println!("ai-setup doctor\n");
    println!("✓ {:10} ai-setup {}", "CLI", env!("CARGO_PKG_VERSION"));
    print_skill_trees(system);
    // Every probe is an independent `<tool> --version`; run them all at once.
    let probes: [(&'static str, &[&str]); 4] = [
        ("node", &["--version"]),
        ("npm", &["--version"]),
        ("pi", &["--version"]),
        ("herdr", &["--version"]),
    ];
    let mut checks = std::thread::scope(|scope| {
        probes
            .map(|(name, args)| scope.spawn(move || check_command(system, name, args)))
            .map(|handle| handle.join().expect("doctor probe thread"))
    });
    // Node is detect-and-instruct only: flag a version below Pi's floor, but
    // never install or update it.
    if let Some(node) = checks.iter_mut().find(|check| check.name == "node") {
        if node.installed {
            if let Some(warning) = crate::NodeStatus::detect(system).warning() {
                node.healthy = false;
                node.detail = format!("{} — {warning}", node.detail);
            }
        }
    }
    for check in &checks {
        let marker = if !check.installed {
            "○"
        } else if check.healthy {
            "✓"
        } else {
            "!"
        };
        println!("{marker} {:10} {}", check.name, check.detail);
    }
    let healthy = checks.iter().all(|check| !check.installed || check.healthy);
    if healthy {
        println!("\nHealthy. Missing managers are installed on demand by `ai-setup setup`.");
    } else {
        println!("\nOne or more installed managers could not run. Repair them, then retry.");
    }
    healthy
}

/// The agent skill trees the native installer would write into, with how
/// many catalog skills each already holds.
fn print_skill_trees(system: &dyn System) {
    let Some(home) = system.home_dir() else {
        println!("! {:10} home directory is unavailable", "skills");
        return;
    };
    let trees = skills::detect_skill_trees(&home);
    if trees.is_empty() {
        println!(
            "○ {:10} no agent directory found ({})",
            "skills",
            skills::agent_dirs_display()
        );
        return;
    }
    let catalog_skills = crate::Catalog::embedded()
        .map(|catalog| {
            catalog
                .resources
                .into_iter()
                .filter(|resource| resource.kind == crate::ResourceKind::Skill)
                .map(|resource| resource.install_target)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for tree in trees {
        let installed = catalog_skills
            .iter()
            .filter(|name| tree.join(name.as_str()).join("SKILL.md").is_file())
            .count();
        println!(
            "✓ {:10} {} ({installed}/{} catalog skills)",
            "skills",
            tree.display(),
            catalog_skills.len()
        );
    }
}

fn check_command(system: &dyn System, name: &'static str, args: &[&str]) -> Check {
    if !system.command_exists(name) {
        return Check {
            name,
            installed: false,
            healthy: true,
            detail: "not installed (optional until selected)".into(),
        };
    }
    let command = CommandSpec::new(name, args.iter().copied());
    match system.run(&command) {
        Ok(result) if result.success => Check {
            name,
            installed: true,
            healthy: true,
            detail: result
                .stdout
                .lines()
                .next()
                .unwrap_or("available")
                .trim()
                .into(),
        },
        Ok(result) => Check {
            name,
            installed: true,
            healthy: false,
            detail: result.stderr.trim().into(),
        },
        Err(error) => Check {
            name,
            installed: true,
            healthy: false,
            detail: error.to_string(),
        },
    }
}
