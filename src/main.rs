mod archive;
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
    /// Collect remote chat archives (currently Codex)
    Glurp {
        /// Host names; omit to collect all configured hosts
        hosts: Vec<String>,
        #[arg(long, default_value = "codex", value_parser = ["codex"])]
        harness: String,
    },
}

#[derive(Subcommand)]
enum HostCommand {
    /// Register a host alias and SSH destination
    Add { name: String, destination: String },
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
            HostCommand::Add { name, destination } => {
                store.add(config::Host { name, destination }, &paths.data)?;
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
            for host in selected {
                match archive::collect(&paths.data, &host, &harness) {
                    Ok(count) => println!("{} {harness}: collected {count} artifacts", host.name),
                    Err(error) => {
                        failed = true;
                        eprintln!("{} {harness}: {error:#}", host.name);
                    }
                }
            }
            if failed {
                bail!("collection failed; check host access and remote Codex source permissions");
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
