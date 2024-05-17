use crate::config::ConfigCli;
use crate::environment::get_up_to_date_prefix;
use crate::progress::await_in_progress;
use crate::project::Environment;
use crate::Project;
use clap::Parser;
use std::path::PathBuf;

/// Install all dependencies
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Install All Environments
    #[arg(long, short)]
    pub all: bool,

    #[clap(flatten)]
    pub lock_file_usage: super::LockFileUsageArgs,

    #[arg(long, short)]
    pub environment: Option<String>,

    #[clap(flatten)]
    pub config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project =
        Project::load_or_else_discover(args.manifest_path.as_deref())?.with_cli_config(args.config);
        
    let envs: Vec<Environment> = if args.all {
        project.environments()
    } else {
        vec![project.environment_from_name_or_env_var(args.environment)?]
    };

    for environment in envs.iter() {
        await_in_progress(
            format!("Installing environment {}...", environment.name()),
            |_| get_up_to_date_prefix(&environment, args.lock_file_usage.into(), false),
        )
        .await?;

        eprintln!(
            "{}Environment {} is ready to use!",
            console::style(console::Emoji("✔ ", "")).green(),
            environment.name()
        );
    }

    eprintln!(
        "{}Project in {} is ready to use!",
        console::style(console::Emoji("✔ ", "")).green(),
        project.root().display()
    );

    Project::warn_on_discovered_from_env(args.manifest_path.as_deref());
    Ok(())
}
