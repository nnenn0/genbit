mod build;
mod config;
mod content;
mod dev;
mod input;
mod markdown;
mod output;
mod route;
mod scaffold;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::net::IpAddr;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a site in a new directory
    New {
        /// Name: ASCII letters, numbers, hyphens, underscores
        name: String,
    },
    /// Build the current site into dist/
    Build,
    /// Build and serve the current site with live reload
    Dev {
        /// Address to bind (use 0.0.0.0 for Docker port forwarding)
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// Port to listen on
        #[arg(long, default_value_t = 3000)]
        port: u16,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::New { name } => {
            scaffold::create(&name)?;
            println!("Created site: {name}");
            println!("Next: cd {name}");
            println!("Run genbit build to generate the site.");
            Ok(())
        }
        Command::Build => {
            let root = std::env::current_dir()?;
            let count = build::run(&root)?;
            println!("Built {count} pages into dist/");
            Ok(())
        }
        Command::Dev { host, port } => {
            let root = std::env::current_dir()?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("cannot start development runtime")?;
            runtime.block_on(dev::run(root, (host, port).into()))
        }
    }
}
