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
    /// Build the current site (not implemented yet)
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
            println!("Scaffolding is ready; build and dev are not implemented yet.");
            Ok(())
        }
        Command::Build => bail!("build is not implemented yet (planned for stage 2)"),
        Command::Dev => bail!("dev is not implemented yet (planned for stage 4)"),
    }
}
