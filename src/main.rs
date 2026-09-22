mod build;
mod content;
mod markdown;
mod output;
mod scaffold;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

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
    /// Serve the current site (not implemented yet)
    Dev,
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
        Command::Dev => bail!("dev is not implemented yet"),
    }
}
