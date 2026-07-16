use crate::{skills, Catalog, CommandSpec, NodeStatus, System};

/// One independent update lane; lanes run concurrently and report whole
/// blocks of output so nothing interleaves.
struct UpdateTask {
    label: &'static str,
    commands: Vec<CommandSpec>,
}

pub fn run_updates(system: &(dyn System + Sync), catalog: &Catalog) -> bool {
    // Warn-only: ai-setup never installs or updates Node itself, but a Node
    // below Pi's floor is worth flagging before Pi's own update runs. A
    // missing Node stays silent here — there is nothing installed to age.
    let node = NodeStatus::detect(system);
    if matches!(node, NodeStatus::TooOld(..)) {
        if let Some(warning) = node.warning() {
            eprintln!("  ! {warning}");
        }
    }

    let mut tasks = Vec::new();
    if system.command_exists("pi") {
        tasks.push(UpdateTask {
            label: "Pi and Pi packages",
            commands: vec![CommandSpec::new("pi", ["update", "--all"])],
        });
    }
    if system.command_exists("herdr") {
        tasks.push(UpdateTask {
            label: "Herdr and Herdr plugins",
            commands: vec![
                CommandSpec::new("herdr", ["update"]),
                CommandSpec::new("herdr", ["plugin", "update", "--all"]),
            ],
        });
    }
    let self_update = if cfg!(windows) {
        CommandSpec::new(
            "powershell",
            [
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                "irm https://raw.githubusercontent.com/Yassimba/ai-setup/main/install.ps1 | iex",
            ],
        )
    } else {
        CommandSpec::new(
            "sh",
            [
                "-c",
                "curl -fsSL https://raw.githubusercontent.com/Yassimba/ai-setup/main/install.sh | sh",
            ],
        )
    };
    tasks.push(UpdateTask {
        label: "AI Setup CLI",
        commands: vec![self_update],
    });

    // Skills, Pi, Herdr, and the self-update touch disjoint state, so all
    // lanes run concurrently; each lane prints once, when it finishes.
    let lanes = std::iter::once("Shared skills")
        .chain(tasks.iter().map(|task| task.label))
        .collect::<Vec<_>>()
        .join(", ");
    println!("Updating in parallel: {lanes}...");
    let mut ready = true;
    let (line_sender, lines) = std::sync::mpsc::channel::<(bool, String)>();
    std::thread::scope(|scope| {
        {
            let line_sender = line_sender.clone();
            scope.spawn(move || {
                let _ = line_sender.send(update_installed_skills(system, catalog));
            });
        }
        for task in &tasks {
            let line_sender = line_sender.clone();
            scope.spawn(move || {
                let _ = line_sender.send(run_update_task(system, task));
            });
        }
        drop(line_sender);
        for (ok, block) in lines {
            ready &= ok;
            if ok {
                println!("{block}");
            } else {
                eprintln!("{block}");
            }
        }
    });
    ready
}

fn run_update_task(system: &dyn System, task: &UpdateTask) -> (bool, String) {
    for command in &task.commands {
        match system.run(command) {
            Ok(result) if result.success => {}
            Ok(result) => {
                return (
                    false,
                    format!(
                        "  ! {}: {}",
                        task.label,
                        crate::install::command_failure_message(&result)
                    ),
                );
            }
            Err(error) => return (false, format!("  ! {}: {error}", task.label)),
        }
    }
    (true, format!("  ✓ {}", task.label))
}

/// Refresh every catalog skill found in any agent tree, into every detected
/// tree. Re-copying the union both updates stale content and backfills agents
/// installed since the skills were: a fresh ~/.codex gets the same skills the
/// other trees already hold.
fn update_installed_skills(system: &dyn System, catalog: &Catalog) -> (bool, String) {
    let Some(home) = system.home_dir() else {
        return (
            false,
            "  ! Shared skills: home directory is unavailable".to_string(),
        );
    };
    let installed = skills::installed_catalog_skills(&catalog.resources, &home);
    if installed.is_empty() {
        return (true, "  ✓ Shared skills (none installed)".to_string());
    }
    let names = skills::expand_skill_dependencies(&catalog.resources, installed)
        .into_iter()
        .map(|resource| resource.install_target)
        .collect::<Vec<_>>();
    match skills::install_skills(system, &names) {
        Ok(reports) => {
            let mut block = String::from("  ✓ Shared skills");
            for report in reports {
                let skipped = if report.skipped_symlinks.is_empty() {
                    String::new()
                } else {
                    format!(" ({} symlinked, left alone)", report.skipped_symlinks.len())
                };
                block.push_str(&format!(
                    "\n      {}: {} skills{skipped}",
                    report.tree.display(),
                    report.installed
                ));
            }
            (true, block)
        }
        Err(message) => (false, format!("  ! Shared skills: {message}")),
    }
}
