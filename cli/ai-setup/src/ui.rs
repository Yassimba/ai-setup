use crate::InstallPlan;
use anyhow::{Context, Result};
use inquire::Confirm;

pub fn print_plan(plan: &InstallPlan) {
    println!("\nInstallation plan:");
    for step in &plan.prerequisites {
        println!("  Prepare {}: {}", step.target, step.action.display());
    }
    for step in &plan.resources {
        println!("  Install {}: {}", step.target, step.action.display());
    }
}

pub fn confirm_plan() -> Result<bool> {
    Confirm::new("Apply this plan?")
        .with_default(false)
        .prompt()
        .context("confirmation was cancelled")
}
