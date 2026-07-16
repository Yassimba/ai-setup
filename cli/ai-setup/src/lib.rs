pub mod app;
mod catalog;
pub mod doctor;
mod install;
mod jsonc;
pub mod settings;
mod skills;
mod system;
mod ui;
pub mod update;
pub mod wizard;

pub use catalog::{Catalog, Resource, ResourceKind};
pub use install::{
    build_install_plan, execute_install_plan, execute_install_plan_with, CommandSpec,
    InstallFailure, InstallPlan, InstallReport, InstallStep, NodeStatus, Platform,
    PrerequisiteStatus, Runtime, StepAction, StepStatus, VerificationSpec, PI_MIN_NODE,
};
pub use skills::{detect_skill_trees, expand_skill_dependencies, installed_catalog_skills};
pub use system::{CommandResult, RealSystem, System};
