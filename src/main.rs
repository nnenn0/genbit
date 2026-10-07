mod build;
mod config;
mod content;
mod content_hash;
mod dev;
mod images;
mod input;
mod markdown;
mod metadata;
mod output;
mod render;
mod route;
mod scaffold;
mod tags;
mod text;

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
    Build {
        /// Run every build step and check, but leave dist/ unchanged
        #[arg(long)]
        dry_run: bool,
    },
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
            println!("Run genbit dev to preview the site.");
            Ok(())
        }
        Command::Build { dry_run } => {
            let root = std::env::current_dir()?;
            let mode = if dry_run {
                build::Mode::DryRun
            } else {
                build::Mode::Publish
            };
            let count = build::run(&root, mode)?;
            if dry_run {
                println!("Checked {count} pages; dist/ was not changed");
            } else {
                println!("Built {count} pages into dist/");
            }
            Ok(())
        }
        Command::Dev { host, port } => {
            let root = std::env::current_dir()?;
            // dev は下書きを含むプレビューをサイトの外に作り、`dist/` を変えない。ランタイムより
            // 先に作るので、停止時に実行中のビルドを待ってから消える。
            let preview = tempfile::Builder::new()
                .prefix("genbit-dev-")
                .tempdir()
                .context("cannot create the preview directory")?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("cannot start development runtime")?;
            runtime.block_on(dev::run(
                root,
                preview.path().join("site"),
                (host, port).into(),
            ))
        }
    }
}
