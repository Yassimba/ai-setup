use ai_setup::app::{install_selected, load_catalog, Selectors};
use ai_setup::doctor::run_doctor;
use ai_setup::update::run_updates;
use ai_setup::RealSystem;
use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use inquire::Confirm;

#[derive(Parser)]
#[command(
    name = "ai-setup",
    version,
    about = "Set up Yassimba's curated skills, Pi packages, and Herdr plugins"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// First-time guided setup
    Setup(SelectionArgs),
    /// Add one or more capabilities
    Add(SelectionArgs),
    /// Update installed managers, packages, plugins, and this CLI
    Update {
        /// Apply updates without confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Check the setup and print actionable repairs
    Doctor,
}

#[derive(Args, Default)]
struct SelectionArgs {
    /// Install a named shared skill; repeat for multiple skills
    #[arg(long = "skill")]
    skills: Vec<String>,
    /// Install a Pi npm package from this catalog; repeat for multiple packages
    #[arg(long = "pi-package")]
    pi_packages: Vec<String>,
    /// Install a Herdr plugin from this catalog; repeat for multiple plugins
    #[arg(long = "herdr-plugin")]
    herdr_plugins: Vec<String>,
    /// Show the plan without making changes
    #[arg(long)]
    dry_run: bool,
    /// Apply the displayed plan without confirmation
    #[arg(long)]
    yes: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let system = RealSystem::default();
    let success = match cli.command {
        Command::Setup(args) | Command::Add(args) => {
            let catalog = load_catalog()?;
            let selectors = Selectors {
                skills: args.skills,
                pi_packages: args.pi_packages,
                herdr_plugins: args.herdr_plugins,
            };
            install_selected(&catalog, &selectors, args.yes, args.dry_run, &system)?
        }
        Command::Doctor => run_doctor(&system),
        Command::Update { yes } => {
            if !yes
                && !Confirm::new("Update installed Yassimba tooling and resources?")
                    .with_default(false)
                    .prompt()?
            {
                println!("Cancelled; no changes made.");
                true
            } else {
                run_updates(&system, &load_catalog()?)
            }
        }
    };
    if !success {
        std::process::exit(1);
    }
    Ok(())
}
