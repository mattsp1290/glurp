use crate::{
    CancellationToken, CollectionControl, CollectionRequest, Harness, InputSource, Limits,
    TaskStatus, archive, collect, config, upgrade,
};
use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version = env!("GLURP_VERSION"), about = "Collect private raw chat archives over OpenSSH")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Replace this binary with a verified GitHub release
    Upgrade {
        /// Report the available release without replacing the binary
        #[arg(long)]
        check: bool,
        /// Replace even a development, Cargo-installed, or current build
        #[arg(long)]
        force: bool,
        /// Install a specific vX.Y.Z release (also supports rollback)
        #[arg(long = "version", value_name = "TAG")]
        tag: Option<String>,
        /// Print the upgrade result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Manage SSH destinations
    Host {
        #[command(subcommand)]
        command: HostCommand,
    },
    /// Restore retained originals locally; may discard a successful newest generation
    Recover {
        host: String,
        /// Explicitly authorize rollback of an ambiguous transaction; never contacts SSH
        #[arg(long, required = true)]
        rollback: bool,
    },
    /// Collect remote chat archives (Claude Code, Codex, pi, OpenCode)
    Glurp {
        /// Host names; omit to collect all configured hosts
        hosts: Vec<String>,
        #[arg(long, value_parser = ["claude", "codex", "pi", "opencode"])]
        harness: Vec<String>,
        #[command(flatten)]
        limits: CliLimits,
    },
}

#[derive(Subcommand)]
enum HostCommand {
    /// Register a host alias and SSH destination
    Add {
        name: String,
        destination: String,
        /// Remote configuration root containing projects/; repeat for multiple roots
        #[arg(long = "claude-path")]
        claude_paths: Vec<String>,
        /// Remote Codex archive root containing sessions/; repeat for multiple roots
        #[arg(long = "codex-path")]
        codex_paths: Vec<String>,
        /// Remote pi session directory; repeat for multiple roots
        #[arg(long = "pi-path")]
        pi_paths: Vec<String>,
    },
    /// List configured hosts
    List,
    /// Remove configuration; preserve collected archives
    Remove { name: String },
}

#[derive(clap::Args)]
struct CliLimits {
    #[arg(long, default_value_t = 256 * 1024 * 1024)]
    max_file_bytes: u64,
    #[arg(long, default_value_t = 100_000)]
    max_files: u64,
    #[arg(long, default_value_t = 1024 * 1024 * 1024)]
    max_total_bytes: u64,
    #[arg(long, default_value_t = 300)]
    timeout_seconds: u64,
}
impl From<CliLimits> for Limits {
    fn from(limits: CliLimits) -> Self {
        Self {
            max_file_bytes: limits.max_file_bytes,
            max_files: limits.max_files,
            max_total_bytes: limits.max_total_bytes,
            timeout_seconds: limits.timeout_seconds,
        }
    }
}

fn collect_command(
    paths: config::Paths,
    hosts: Vec<String>,
    harnesses: Vec<String>,
    limits: CliLimits,
) -> Result<()> {
    let request = CollectionRequest {
        source: InputSource::Stored {
            config_dir: paths
                .config
                .parent()
                .ok_or_else(|| anyhow::anyhow!("config parent missing"))?
                .to_owned(),
            archive_root: paths.data,
            hosts,
        },
        harnesses: harnesses
            .into_iter()
            .map(|harness| match harness.as_str() {
                "claude" => Harness::Claude,
                "codex" => Harness::Codex,
                "opencode" => Harness::OpenCode,
                "pi" => Harness::Pi,
                _ => unreachable!("clap validates harness selection"),
            })
            .collect(),
        limits: limits.into(),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let report = runtime.block_on(async {
        // Only command-line execution installs process signal listeners.
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let cancellation = CancellationToken::new();
        let signal_token = cancellation.clone();
        let listener = tokio::spawn(async move {
            tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} }
            signal_token.cancel();
        });
        let result = collect(
            request,
            CollectionControl {
                cancellation,
                ..Default::default()
            },
        )
        .await;
        listener.abort();
        let _ = listener.await;
        result.map_err(anyhow::Error::from)
    })?;
    let success = report.is_success();
    for task in report.tasks {
        let reason = match task.status {
            TaskStatus::Collected { artifact_count, .. } => {
                println!(
                    "{} {}: collected {artifact_count} artifacts",
                    task.host, task.harness
                );
                continue;
            }
            TaskStatus::NotFound => {
                println!("{} {}: not-found", task.host, task.harness);
                continue;
            }
            TaskStatus::Busy => "archive in use; retry later".to_owned(),
            TaskStatus::Failed { kind } => kind.to_string(),
            TaskStatus::Cancelled | TaskStatus::NotStartedDueToCancellation => {
                "collection cancelled".to_owned()
            }
        };
        let guidance = match task.harness {
            Harness::Claude => "check --claude-path, CLAUDE_CONFIG_DIR and source permissions",
            Harness::Codex => "check --codex-path, CODEX_HOME and source permissions",
            Harness::Pi => {
                "check --pi-path, PI_CODING_AGENT_SESSION_DIR/PI_CODING_AGENT_DIR and source permissions"
            }
            Harness::OpenCode => {
                "check OpenCode database access and inventory/export compatibility"
            }
        };
        eprintln!(
            "{} {}: {reason}; {guidance}; check configured limits, timeout and local storage",
            task.host, task.harness
        );
    }
    if !success {
        bail!("collection failed; check host access and remote harness source permissions");
    }
    Ok(())
}

fn run(cli: Cli) -> Result<()> {
    // An upgrade must not open configuration, acquire archive locks or contact SSH.
    if let Commands::Upgrade {
        check,
        force,
        tag,
        json,
    } = &cli.command
    {
        return upgrade::execute(*check, *force, tag.as_deref(), *json);
    }
    let paths = config::Paths::resolve()?;
    if let Commands::Glurp {
        hosts,
        harness,
        limits,
    } = cli.command
    {
        return collect_command(paths, hosts, harness, limits);
    }
    let mut store = config::Store::open(&paths.config)?;
    match cli.command {
        Commands::Upgrade { .. } => unreachable!("upgrade is handled before opening storage"),
        Commands::Host { command } => match command {
            HostCommand::Add {
                name,
                destination,
                claude_paths,
                codex_paths,
                pi_paths,
            } => {
                store.add(
                    config::Host {
                        name,
                        destination,
                        claude_paths,
                        codex_paths,
                        pi_paths,
                    },
                    &paths.data,
                )?;
                println!("host added");
            }
            HostCommand::List => {
                if store.hosts.is_empty() {
                    println!("no hosts configured; use glurp host add <name> <destination>");
                }
                for host in &store.hosts {
                    println!("{}\t{}", host.name, host.destination);
                }
            }
            HostCommand::Remove { name } => {
                store.remove(&name)?;
                println!("host removed; archives retained");
            }
        },
        Commands::Recover { host, rollback: _ } => {
            let selected = store.select(&[host])?;
            drop(store);
            archive::rollback(&paths.data, &selected[0])?;
            println!("local rollback complete; originals restored");
        }
        Commands::Glurp { .. } => unreachable!("collection is handled before opening storage"),
    }
    Ok(())
}

pub(crate) fn main() -> i32 {
    main_from(std::env::args_os())
}

fn main_from<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return code;
        }
    };
    if let Err(error) = run(cli) {
        eprintln!("glurp: {error:#}");
        return 1;
    }
    0
}

#[cfg(test)]
mod tests;
