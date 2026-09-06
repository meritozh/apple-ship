use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::commands::{ci, doctor, setup, ship};
use crate::policy::Channel;

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Doctor { channel } => doctor::run(channel),
        Commands::Setup {
            certs,
            api_key,
            api_issuer,
            apple_id,
            password_stdin,
            force,
            push,
        } => setup::run(setup::Options {
            certs,
            api_key,
            api_issuer,
            apple_id,
            password_stdin,
            force,
            push,
        }),
        Commands::Ship { channel } => ship::run(channel),
        Commands::Ci { channel } => ci::run(channel),
    }
}

#[derive(Parser)]
#[command(
    name = "apple-ship",
    about = "Ship macOS Tauri, GPUI, and native apps from GitHub Actions",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check config, workflow, and GitHub secrets (does not sign locally)
    Doctor {
        #[arg(long, value_enum)]
        channel: Option<Channel>,
    },
    /// Point at certificates; write workflow and GitHub environment secrets
    Setup {
        /// PKCS#12 (.p12) preferred. Repeatable. Role is detected from the cert.
        #[arg(long = "cert", required = true, action = clap::ArgAction::Append)]
        certs: Vec<PathBuf>,
        /// App Store Connect API private key (AuthKey_XXX.p8)
        #[arg(long)]
        api_key: Option<PathBuf>,
        /// App Store Connect issuer UUID (required with --api-key)
        #[arg(long)]
        api_issuer: Option<String>,
        /// Apple ID email for notarytool (alternative to --api-key)
        #[arg(long)]
        apple_id: Option<String>,
        /// Read a single PKCS#12 password from stdin (otherwise prompt per file)
        #[arg(long)]
        password_stdin: bool,
        /// Overwrite an existing workflow / apple-ship.toml
        #[arg(long)]
        force: bool,
        /// Commit and push the generated workflow
        #[arg(long)]
        push: bool,
    },
    /// Dispatch the macOS Ship workflow and watch it
    Ship {
        #[arg(long, value_enum)]
        channel: Channel,
    },
    /// Build, sign, and notarize. Only runs on GitHub Actions.
    Ci {
        #[arg(long, value_enum)]
        channel: Channel,
    },
}
