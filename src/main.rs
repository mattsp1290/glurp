mod archive;
mod collectors;
mod config;
mod limits;
mod remote;
mod secure;
mod transaction;

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
        limits: limits::Limits,
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

fn failure_reason(error: &anyhow::Error) -> &'static str {
    // Only fixed local categories are printed. Never expose parser excerpts,
    // remote artifact names, transcript bytes, or underlying SSH diagnostics.
    let message = error.to_string();
    if message.contains("archive is in use") {
        "archive in use; retry later"
    } else if message.contains("bound to a different") || message.contains("binding is missing") {
        "archive destination binding refused"
    } else if message.contains("timed out") {
        "operation timed out"
    } else if message.contains("cancelled") {
        "collection cancelled"
    } else if message.contains("recovery")
        || message.contains("rollback")
        || message.contains("backups retained")
    {
        "local recovery required; backups retained"
    } else if message.contains("limit") || message.contains("overflow") {
        "archive limit or size overflow"
    } else if message.contains("frame")
        || message.contains("protocol")
        || message.contains("truncated")
        || message.contains("unsafe artifact")
        || message.contains("duplicate artifact")
    {
        "invalid remote archive stream"
    } else {
        "collection failed"
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    remote::install_cancellation()?;
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
        Commands::Recover { host, rollback: _ } => {
            let selected = store.select(&[host])?;
            drop(store);
            archive::rollback(&paths.data, &selected[0])?;
            println!("local rollback complete; originals restored");
        }
        Commands::Glurp {
            hosts,
            harness,
            limits,
        } => {
            limits.wire_limit()?;
            let selected = store.select(&hosts)?;
            // Release config lock before potentially slow SSH operations.
            drop(store);
            let mut failed = false;
            let mut harnesses = if harness.is_empty() {
                vec![
                    "claude".into(),
                    "codex".into(),
                    "pi".into(),
                    "opencode".into(),
                ]
            } else {
                harness
            };
            harnesses.sort();
            harnesses.dedup();
            for host in selected {
                if remote::cancelled() {
                    bail!("collection cancelled");
                }
                for harness in &harnesses {
                    if remote::cancelled() {
                        bail!("collection cancelled");
                    }
                    match archive::collect(&paths.data, &host, harness, &limits) {
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
                            eprintln!(
                                "{} {harness}: {}; {guidance}; check configured limits, timeout and local storage",
                                host.name,
                                failure_reason(&error)
                            );
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
