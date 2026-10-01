mod archive;
mod collectors;
mod config;
mod remote;
mod secure;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Collect private raw chat archives over OpenSSH")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage SSH destinations
    Host {
        #[command(subcommand)]
        command: HostCommand,
    },
    /// Collect remote chat archives (Claude Code, Codex, pi, OpenCode)
    Glurp {
        /// Host names; omit to collect all configured hosts
        hosts: Vec<String>,
        #[arg(long, value_parser = ["claude", "codex", "pi", "opencode"])]
        harness: Vec<String>,
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

fn run() -> Result<()> {
    let cli = Cli::parse();
    let paths = config::Paths::resolve()?;
    let mut store = config::Store::open(&paths.config)?;
    match cli.command {
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
        Commands::Glurp { hosts, harness } => {
            let selected = store.select(&hosts)?;
            // Release config lock before potentially slow SSH operations.
            drop(store);
            let mut failed = false;
            let harnesses = if harness.is_empty() {
                vec![
                    "claude".into(),
                    "codex".into(),
                    "pi".into(),
                    "opencode".into(),
                ]
            } else {
                harness
            };
            for host in selected {
                for harness in &harnesses {
                    match archive::collect(&paths.data, &host, harness) {
                        Ok(result) => println!("{} {harness}: {result}", host.name),
                        Err(error) => {
                            failed = true;
                            let guidance = match harness.as_str() {
                                "claude" => {
                                    "check --claude-path, CLAUDE_CONFIG_DIR and source permissions"
                                }
                                "codex" => "check --codex-path, CODEX_HOME and source permissions",
                                "pi" => {
                                    "check --pi-path, PI_CODING_AGENT_SESSION_DIR/PI_CODING_AGENT_DIR and source permissions"
                                }
                                _ => {
                                    "check OpenCode database access and inventory/export compatibility"
                                }
                            };
                            eprintln!("{} {harness}: {error:#}; {guidance}", host.name);
                        }
                    }
                }
            }
            if failed {
                bail!("collection failed; check host access and remote harness source permissions");
            }
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("glurp: {error:#}");
        std::process::exit(1);
    }
}
