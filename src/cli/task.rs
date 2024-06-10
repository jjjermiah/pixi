use crate::project::manifest::EnvironmentName;
use crate::project::manifest::FeatureName;
use crate::project::virtual_packages::verify_current_platform_has_required_virtual_packages;
use crate::project::Environment;
use crate::task::{quote, Alias, CmdArgs, Execute, Task, TaskName};
use crate::Project;
use clap::Parser;
use indexmap::IndexMap;
use itertools::Itertools;
use rattler_conda_types::Platform;
use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::io;
use std::io::{stdout, Write};
use std::path::PathBuf;
use std::str::FromStr;
use toml_edit::{Array, Item, Table, Value};

#[derive(Parser, Debug)]
pub enum Operation {
    /// Add a command to the project
    #[clap(visible_alias = "a")]
    Add(AddArgs),

    /// Remove a command from the project
    // BREAK: This should only have the `rm` alias
    #[clap(visible_alias = "rm", alias = "r")]
    Remove(RemoveArgs),

    /// Alias another specific command
    #[clap(alias = "@")]
    Alias(AliasArgs),

    /// List all tasks in the project
    #[clap(visible_alias = "ls", alias = "l")]
    List(ListArgs),
}

#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct RemoveArgs {
    /// Task names to remove
    pub names: Vec<TaskName>,

    /// The platform for which the task should be removed
    #[arg(long, short)]
    pub platform: Option<Platform>,

    /// The feature for which the task should be removed
    #[arg(long, short)]
    pub feature: Option<String>,
}

#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true)]
pub struct AddArgs {
    /// Task name
    pub name: TaskName,

    /// One or more commands to actually execute
    #[clap(required = true, num_args = 1..)]
    pub commands: Vec<String>,

    /// Depends on these other commands
    #[clap(long)]
    #[clap(num_args = 1..)]
    pub depends_on: Option<Vec<TaskName>>,

    /// The platform for which the task should be added
    #[arg(long, short)]
    pub platform: Option<Platform>,

    /// The feature for which the task should be added
    #[arg(long, short)]
    pub feature: Option<String>,

    /// The working directory relative to the root of the project
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    /// The environment variable to set, use --env key=value multiple times for more than one variable
    #[arg(long, value_parser = parse_key_val)]
    pub env: Vec<(String, String)>,

    /// A description of the task to be added.
    #[arg(long)]
    pub description: Option<String>,
}

/// Parse a single key-value pair
fn parse_key_val(s: &str) -> Result<(String, String), Box<dyn Error + Send + Sync + 'static>> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{}`", s))?;
    let key = s[..pos].to_string();
    let value = s[pos + 1..].to_string();
    Ok((key, value))
}

#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true)]
pub struct AliasArgs {
    /// Alias name
    pub alias: TaskName,

    /// Depends on these tasks to execute
    #[clap(required = true, num_args = 1..)]
    pub depends_on: Vec<TaskName>,

    /// The platform for which the alias should be added
    #[arg(long, short)]
    pub platform: Option<Platform>,

    /// The description of the alias task
    #[arg(long)]
    pub description: Option<String>,
}

#[derive(Parser, Debug, Clone)]
pub struct ListArgs {
    /// Tasks available for this machine per environment
    #[arg(long, short)]
    pub summary: bool,

    /// Output the list of tasks from all environments in
    /// machine readable format (space delimited)
    /// this output is used for autocomplete by `pixi run`
    #[arg(long, hide(true))]
    pub machine_readable: bool,

    /// The environment the list should be generated for.
    /// If not specified, the default environment is used.
    #[arg(long, short)]
    pub environment: Option<String>,
}

impl From<AddArgs> for Task {
    fn from(value: AddArgs) -> Self {
        let depends_on = value.depends_on.unwrap_or_default();
        // description or none
        let description = value.description;

        // Convert the arguments into a single string representation
        let cmd_args = if value.commands.len() == 1 {
            value.commands.into_iter().next().unwrap()
        } else {
            // Simply concatenate all arguments
            value
                .commands
                .into_iter()
                .map(|arg| quote(&arg).into_owned())
                .join(" ")
        };

        // Depending on whether the task has a command, and depends_on or not we create a plain or
        // complex, or alias command.
        if cmd_args.trim().is_empty() && !depends_on.is_empty() {
            Self::Alias(Alias {
                depends_on,
                description,
            })
        } else if depends_on.is_empty()
            && value.cwd.is_none()
            && value.env.is_empty()
            && description.is_none()
        {
            Self::Plain(cmd_args)
        } else {
            let cwd = value.cwd;
            let env = if value.env.is_empty() {
                None
            } else {
                let mut env = IndexMap::new();
                for (key, value) in value.env {
                    env.insert(key, value);
                }
                Some(env)
            };

            Self::Execute(Execute {
                cmd: CmdArgs::Single(cmd_args),
                depends_on,
                inputs: None,
                outputs: None,
                cwd,
                env,
                description,
            })
        }
    }
}

impl From<AliasArgs> for Task {
    fn from(value: AliasArgs) -> Self {
        Self::Alias(Alias {
            depends_on: value.depends_on,
            description: value.description,
        })
    }
}

/// Interact with tasks in the project
#[derive(Parser, Debug)]
#[clap(trailing_var_arg = true, arg_required_else_help = true)]
pub struct Args {
    /// Add, remove, or update a task
    #[clap(subcommand)]
    pub operation: Operation,

    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

fn print_heading(value: &str) {
    let bold = console::Style::new().bold();
    eprintln!("{}\n{:-<2$}", bold.apply_to(value), "", value.len(),);
}

fn print_tasks_per_env(envs: Vec<Environment>) -> io::Result<()> {
    let mut writer = tabwriter::TabWriter::new(stdout());
    for env in envs {
        let formatted: String = env
            .get_filtered_tasks()
            .iter()
            .sorted()
            .map(|name| name.fancy_display())
            .join(", ");
        writeln!(
            writer,
            "{}\t: {}",
            env.name().fancy_display().bold(),
            formatted
        )?;
    }
    writer.flush()?;
    Ok(())
}

fn get_feature_tasks_per_env(
    task_list: HashSet<TaskName>,
    environments: Vec<Environment>,
) -> HashMap<Environment, HashMap<FeatureName, IndexMap<TaskName, Task>>> {
    let mut task_env_feat_map: HashMap<
        Environment,
        HashMap<FeatureName, IndexMap<TaskName, Task>>,
    > = HashMap::new();

    for env in environments {
        let feature_tasks = env.feature_tasks();
        for (feature_name, feature_tasks) in feature_tasks {
            for taskname in task_list.iter() {
                // if the task is in the feature_tasks keys, add it to the hashmap for that feature
                if let Some(task) = feature_tasks.get(&taskname.clone()).cloned() {
                    let task_map = task_env_feat_map
                        .entry(env.clone())
                        .or_insert_with(HashMap::new)
                        .entry(feature_name.clone())
                        .or_insert_with(IndexMap::new);
                    task_map.insert(taskname.clone(), task.clone());
                }
            }
        }
    }
    task_env_feat_map
}

fn get_tasks_per_env(
    task_list: HashSet<TaskName>,
    environments: Vec<Environment>,
) -> HashMap<Environment, HashMap<TaskName, Task>> {
    let mut tasks_per_env: HashMap<Environment, HashMap<TaskName, Task>> = HashMap::new();
    for env in environments {
        let mut tasks: HashMap<TaskName, Task> = HashMap::new();
        let this_env_tasks = env
            .tasks(Some(env.best_platform()))
            .expect("error getting tasks");
        for taskname in task_list.iter() {
            // if the task is in the environment, add it to the list
            if let Some(&task) = this_env_tasks.get(taskname) {
                tasks.insert(taskname.clone(), task.clone());
            }
        }
        tasks_per_env.insert(env, tasks);
    }
    tasks_per_env
}

fn print_task_per_feat_descriptions(
    tasks_per_env: HashMap<Environment, HashMap<FeatureName, IndexMap<TaskName, Task>>>,
) -> io::Result<()> {
    // construct a formatted string for each task across all features and environments:
    // should look like "taskname (featurename) : description"
    // Add to a set so they are ALL unique and then print them
    let mut task_descriptions: HashSet<String> = HashSet::new();
    for (_, feature_tasks) in tasks_per_env {
        for (feature_name, task) in feature_tasks {
            for (task_name, task) in task {
                if let Some(description) = task.description() {
                    // tabular columns for each
                    task_descriptions.insert(format!(
                        "{:<15} {:<15} {:<50}",
                        task_name.fancy_display(),
                        feature_name.fancy_display(),
                        description,
                    ));
                }
            }
        }
    }
    let mut writer = tabwriter::TabWriter::new(stdout());
    // print an empty line
    writeln!(writer, "").expect("io error when printing empty line");
    print_heading(format!("{:<15} {:<15} {:<50}", "Task", "Feature", "Description").as_str());

    let mut sorted_task_descs: Vec<String> = task_descriptions.iter().cloned().collect();
    sorted_task_descs.sort();

    for task in sorted_task_descs {
        writeln!(writer, "{}", task).expect("io error when printing task");
    }

    writer.flush().expect("io error when clearing tasks");
    Ok(())
}

fn print_tasks_per_env_descriptions(
    tasks_per_env: HashMap<Environment, HashMap<TaskName, Task>>,
) -> io::Result<()> {
    // construct a formatted string for each task across all features and environments:
    // should look like "taskname (environment) : description"
    // Add to a set so they are ALL unique and then print them
    let mut task_descriptions: HashSet<String> = HashSet::new();
    for (env, task) in tasks_per_env {
        for (task_name, task) in task {
            if let Some(description) = task.description() {
                // tabular columns for each
                task_descriptions.insert(format!(
                    "{:<15} {:<15} {:<50}",
                    task_name.fancy_display(),
                    env.name().fancy_display(),
                    description,
                ));
            }
        }
    }
    let mut writer = tabwriter::TabWriter::new(stdout());
    // print an empty line
    writeln!(writer, "").expect("io error when printing empty line");
    print_heading(format!("{:<15} {:<15} {:<50}", "Task", "Environment", "Description").as_str());

    let mut sorted_task_descs: Vec<String> = task_descriptions.iter().cloned().collect();
    sorted_task_descs.sort();

    for task in sorted_task_descs {
        writeln!(writer, "{}", task).expect("io error when printing task");
    }

    writer.flush().expect("io error when clearing tasks");
    Ok(())
}

pub fn execute(args: Args) -> miette::Result<()> {
    let mut project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
    match args.operation {
        Operation::Add(args) => {
            let name = &args.name;
            let task: Task = args.clone().into();
            let feature = args
                .feature
                .map_or(FeatureName::Default, FeatureName::Named);
            project
                .manifest
                .add_task(name.clone(), task.clone(), args.platform, &feature)?;
            project.save()?;
            eprintln!(
                "{}Added task `{}`: {}",
                console::style(console::Emoji("✔ ", "+")).green(),
                name.fancy_display().bold(),
                task,
            );
        }
        Operation::Remove(args) => {
            let mut to_remove = Vec::new();
            let feature = args
                .feature
                .map_or(FeatureName::Default, FeatureName::Named);
            for name in args.names.iter() {
                if let Some(platform) = args.platform {
                    if !project
                        .manifest
                        .tasks(Some(platform), &feature)?
                        .contains_key(name)
                    {
                        eprintln!(
                            "{}Task '{}' does not exist on {}",
                            console::style(console::Emoji("❌ ", "X")).red(),
                            name.fancy_display().bold(),
                            console::style(platform.as_str()).bold(),
                        );
                        continue;
                    }
                } else if !project.manifest.tasks(None, &feature)?.contains_key(name) {
                    eprintln!(
                        "{}Task `{}` does not exist for the `{}` feature",
                        console::style(console::Emoji("❌ ", "X")).red(),
                        name.fancy_display().bold(),
                        console::style(&feature).bold(),
                    );
                    continue;
                }

                // Check if task has dependencies
                // TODO: Make this properly work by inspecting which actual tasks depend on the task
                //  we just removed taking into account environments and features.
                // let depends_on = project.task_names_depending_on(name);
                // if !depends_on.is_empty() && !args.names.contains(name) {
                //     eprintln!(
                //         "{}: {}",
                //         console::style("Warning, the following task/s depend on this task")
                //             .yellow(),
                //         console::style(depends_on.iter().to_owned().join(", ")).bold()
                //     );
                //     eprintln!(
                //         "{}",
                //         console::style("Be sure to modify these after the removal\n").yellow()
                //     );
                // }

                // Safe to remove
                to_remove.push((name, args.platform));
            }

            for (name, platform) in to_remove {
                project
                    .manifest
                    .remove_task(name.clone(), platform, &feature)?;
                project.save()?;
                eprintln!(
                    "{}Removed task `{}` ",
                    console::style(console::Emoji("✔ ", "+")).green(),
                    name.fancy_display().bold(),
                );
            }
        }
        Operation::Alias(args) => {
            let name = &args.alias;
            let task: Task = args.clone().into();
            project.manifest.add_task(
                name.clone(),
                task.clone(),
                args.platform,
                &FeatureName::Default,
            )?;
            project.save()?;
            eprintln!(
                "{} Added alias `{}`: {}",
                console::style("@").blue(),
                name.fancy_display().bold(),
                task,
            );
        }
        Operation::List(args) => {
            let explicit_environment = args
                .environment
                .map(|n| EnvironmentName::from_str(n.as_str()))
                .transpose()?
                .map(|n| {
                    project
                        .environment(&n)
                        .ok_or_else(|| miette::miette!("unknown environment '{n}'"))
                })
                .transpose()?;
            let available_tasks: HashSet<TaskName> =
                if let Some(explicit_environment) = explicit_environment {
                    explicit_environment.get_filtered_tasks()
                } else {
                    project
                        .environments()
                        .into_iter()
                        .filter(|env| {
                            verify_current_platform_has_required_virtual_packages(env).is_ok()
                        })
                        .flat_map(|env| env.get_filtered_tasks())
                        .collect()
                };

            if available_tasks.is_empty() {
                eprintln!("No tasks found",);
                return Ok(());
            } else if args.machine_readable {
                let unformatted: String = available_tasks
                    .iter()
                    .sorted()
                    .map(|name| name.as_str())
                    .join(" ");
                eprintln!("{}", unformatted);
                return Ok(());
            }

            let tasks_per_env = get_tasks_per_env(available_tasks.clone(), project.environments());
            let tasks_per_env_feat =
                get_feature_tasks_per_env(available_tasks.clone(), project.environments());
            if args.summary {
                print_heading("Tasks per environment:");
                print_tasks_per_env(project.environments()).expect("io error when printing tasks");
            } else {
                let formatted_tasks: String = available_tasks
                    .iter()
                    .sorted()
                    .map(|name| name.fancy_display())
                    .join(", ");
                print_heading("Tasks that can run on this machine:");
                eprintln!("{}", formatted_tasks);

                let mut writer = tabwriter::TabWriter::new(stdout());
                let mut tasks_and_desc: HashSet<String> = HashSet::new();

                for (_, tasks) in tasks_per_env.iter() {
                    for (task_name, task) in tasks {
                        if let Some(description) = task.description() {
                            tasks_and_desc.insert(format!(
                                " - {:<15} {:<50}",
                                task_name.fancy_display(),
                                description,
                            ));
                        }
                    }
                }

                // print an empty line
                writeln!(writer, "").expect("io error when printing empty line");

                let mut sorted_task_descs: Vec<String> = tasks_and_desc.iter().cloned().collect();
                sorted_task_descs.sort();

                for task in sorted_task_descs {
                    writeln!(writer, "{}", task).expect("io error when printing task");
                }

                //
                writeln!(writer, "").expect("io error when printing empty line");

                writeln!(writer, "with redundant tasknames").expect("io error when printing task");
                // using the tasks_per_env hashmap, get all the duplicated tasks and print them
                let mut task_counts: HashMap<TaskName, usize> = HashMap::new();

                // Count occurrences of each task name
                for tasks in tasks_per_env.values() {
                    for task_name in tasks.keys() {
                        *task_counts.entry(task_name.clone()).or_insert(0) += 1;
                    }
                }

                let mut new_tasks_and_desc: IndexMap<TaskName, String> = IndexMap::new();
                // do the same as the for loop below, except if a taskname is duplicated, add (env) to the end
                for (env, tasks) in tasks_per_env.iter() {
                    for (task_name, task) in tasks {
                        if let Some(description) = task.description() {
                            // the idea here is to populate the new_tasks_and_desc hashmap with the taskname and description
                            // but if the taskname is duplicated, append the environment name to the end of the description
                            // so if the task "build" is in multiple environments, it will print as " - TASKNAME : DESCRIPTION (ENV, ENV2)"
                            let mut desc =
                                format!(" - {:<15} {}", task_name.fancy_display(), description,);
                            if task_counts[task_name] > 1 {
                                // first check if the taskname is already in the indexmap and append the environment name
                                if let Some(existing_desc) = new_tasks_and_desc.get_mut(task_name) {
                                    *existing_desc = format!(
                                        "{} ({})",
                                        existing_desc,
                                        env.name().fancy_display()
                                    );
                                } else {
                                    desc = format!("{} ({})", desc, env.name().fancy_display());
                                    new_tasks_and_desc.insert(task_name.clone(), desc);
                                }
                            } else {
                                new_tasks_and_desc.insert(task_name.clone(), desc);
                            }
                        }
                    }
                }
                let mut sorted_new_tasks_and_desc: IndexMap<TaskName, String> =
                    new_tasks_and_desc.clone();
                sorted_new_tasks_and_desc.sort_keys();

                for (taskname, desc) in sorted_new_tasks_and_desc {
                    writeln!(writer, "{}", desc).expect("io error when printing task");
                }
                print_tasks_per_env_descriptions(tasks_per_env)
                    .expect("io error when printing task descriptions");
                print_task_per_feat_descriptions(tasks_per_env_feat)
                    .expect("io error when printing task descriptions");
            }
        }
    };

    Project::warn_on_discovered_from_env(args.manifest_path.as_deref());
    Ok(())
}

impl From<Task> for Item {
    fn from(value: Task) -> Self {
        match value {
            Task::Plain(str) => Item::Value(str.into()),
            Task::Execute(process) => {
                let mut table = Table::new().into_inline_table();
                match process.cmd {
                    CmdArgs::Single(cmd_str) => {
                        table.insert("cmd", cmd_str.into());
                    }
                    CmdArgs::Multiple(cmd_strs) => {
                        table.insert("cmd", Value::Array(Array::from_iter(cmd_strs)));
                    }
                }
                if !process.depends_on.is_empty() {
                    table.insert(
                        "depends-on",
                        Value::Array(Array::from_iter(
                            process
                                .depends_on
                                .into_iter()
                                .map(String::from)
                                .map(Value::from),
                        )),
                    );
                }
                if let Some(cwd) = process.cwd {
                    table.insert("cwd", cwd.to_string_lossy().to_string().into());
                }
                if let Some(env) = process.env {
                    table.insert("env", Value::InlineTable(env.into_iter().collect()));
                }
                if let Some(description) = process.description {
                    table.insert("description", description.into());
                }
                Item::Value(Value::InlineTable(table))
            }
            Task::Alias(alias) => {
                let mut table = Table::new().into_inline_table();
                table.insert(
                    "depends-on",
                    Value::Array(Array::from_iter(
                        alias
                            .depends_on
                            .into_iter()
                            .map(String::from)
                            .map(Value::from),
                    )),
                );
                Item::Value(Value::InlineTable(table))
            }
            _ => Item::None,
        }
    }
}
