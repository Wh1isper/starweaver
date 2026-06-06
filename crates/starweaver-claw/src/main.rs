//! Starweaver Claw command-line entry point.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use starweaver_claw::{migrate_sqlite_database, serve, ClawResult, ClawSettings};

/// Starweaver Claw service command.
#[derive(Debug, Parser)]
#[command(
    name = "starweaver-claw",
    version,
    about = "Run the Starweaver Claw service"
)]
struct Cli {
    /// Command to run.
    #[command(subcommand)]
    command: Option<Command>,
}

/// Supported service commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Start the HTTP service.
    Start(StartArgs),
    /// Apply pending database migrations and exit.
    Migrate(MigrateArgs),
    /// Print resolved service configuration.
    Config,
}

/// Start command options.
#[derive(Debug, Parser)]
struct StartArgs {
    /// Optional TOML config path.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Override bind host.
    #[arg(long)]
    host: Option<String>,
    /// Override bind port.
    #[arg(long)]
    port: Option<u16>,
}

/// Database migration options.
#[derive(Debug, Parser)]
struct MigrateArgs {
    /// Optional TOML config path.
    #[arg(long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> ClawResult<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Start(StartArgs {
        config: None,
        host: None,
        port: None,
    })) {
        Command::Start(args) => {
            let mut settings = load_settings(args.config)?;
            if let Some(host) = args.host {
                settings.host = host;
            }
            if let Some(port) = args.port {
                settings.port = port;
            }
            serve(settings).await
        }
        Command::Migrate(args) => {
            let settings = load_settings(args.config)?;
            settings.ensure_dirs()?;
            let applied = migrate_sqlite_database(&settings.sqlite_path)?;
            println!(
                "{{\"database\":{},\"applied_migrations\":{}}}",
                serde_json::to_string(&settings.sqlite_path)?,
                serde_json::to_string(&applied)?,
            );
            Ok(())
        }
        Command::Config => {
            let settings = ClawSettings::from_env();
            println!("{}", serde_json::to_string_pretty(&settings)?);
            Ok(())
        }
    }
}

fn load_settings(config: Option<PathBuf>) -> ClawResult<ClawSettings> {
    config.map_or_else(|| Ok(ClawSettings::from_env()), ClawSettings::from_file)
}
