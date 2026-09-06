use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::commands::{ci, release, setup};
use crate::policy::{Channel, Kind};

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Setup {
            channel,
            cert,
            kind,
            password_stdin,
            apple_id,
            api_key,
            api_issuer,
        } => setup::run(setup::Options {
            channel,
            cert,
            kind,
            password_stdin,
            apple_id,
            api_key,
            api_issuer,
        }),
        Commands::Release { spec } => release::run(&spec),
        Commands::Ci { channel } => ci::run(channel),
    }
}

#[derive(Parser)]
#[command(
    name = "apple-ship",
    about = "One-command GitHub Actions CI setup for shipping a macOS app",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Configure this repository's GitHub Actions shipping workflow
    Setup {
        /// developer-id (direct distribution) or app-store
        #[arg(long, value_enum)]
        channel: Channel,
        /// PKCS#12 file, or a folder of .cer/.p12 files (channel selects the matching cert)
        #[arg(long)]
        cert: PathBuf,
        /// Skip auto-detect and generate apple-ship.toml for this kind
        #[arg(long, value_enum)]
        kind: Option<Kind>,
        /// Read the PKCS#12 password from stdin instead of prompting
        #[arg(long)]
        password_stdin: bool,
        /// Apple ID email for notarytool (Developer ID). Prompted if omitted.
        #[arg(long)]
        apple_id: Option<String>,
        /// App Store Connect API key .p8 (alternative to Apple ID)
        #[arg(long)]
        api_key: Option<PathBuf>,
        /// App Store Connect issuer UUID (with --api-key)
        #[arg(long)]
        api_issuer: Option<String>,
    },
    /// Bump version, commit, tag, and push to trigger CI
    Release {
        /// vX.Y.Z, or major / minor / patch
        spec: String,
    },
    /// Build, sign, and notarize. Only runs on GitHub Actions.
    Ci {
        #[arg(long, value_enum)]
        channel: Channel,
    },
}
