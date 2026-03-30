mod api;
mod capture;
mod cli;
mod config;
mod daemon;
mod embedding;
mod ocr;
mod plugin;
mod storage;

use clap::{Parser, Subcommand};

use crate::config::Config;

#[derive(Parser)]
#[command(name = "recalld", version, about = "Linux screen recall daemon")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the daemon (capture loop + gRPC server)
    Daemon {
        /// Passphrase for encrypting/decrypting stored data.
        /// If omitted, reads from RECALLD_PASSPHRASE env var or prompts on stdin.
        #[arg(long, env = "RECALLD_PASSPHRASE")]
        passphrase: Option<String>,
    },
    /// Search stored screenshots by semantic query
    Search {
        /// The search query
        query: String,
        /// Max number of results
        #[arg(short, long, default_value = "20")]
        limit: u32,
    },
    /// Show daemon status
    Status,
    /// Manage plugins
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
    /// Write config file to stdout 
    Config {
        #[arg(long)]
        stdout: bool,
        path: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
enum PluginAction {
    /// List all discovered plugins
    List,
    /// Enable a plugin by name
    Enable { name: String },
    /// Disable a plugin by name
    Disable { name: String },
}

enum ConfigAction {
    /// Write config file to stdout
    Stdout,
    /// Write config file to specified path
    Path(std::path::PathBuf),
}

fn main() -> anyhow::Result<()> {
    // Initialise tracing (respects RUST_LOG env)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let config = config::Config::load()?;

    // Limit thread counts for all math libraries before spawning any threads.
    // SAFETY: called in main() before the tokio runtime is created — no other threads exist.
    let threads = config.processing.embedding_threads.to_string();
    unsafe {
        std::env::set_var("OMP_NUM_THREADS", &threads);
        std::env::set_var("OMP_THREAD_LIMIT", &threads);
        std::env::set_var("MKL_NUM_THREADS", &threads);
        std::env::set_var("OPENBLAS_NUM_THREADS", &threads);
        std::env::set_var("RAYON_NUM_THREADS", &threads);
    }

    tokio::runtime::Runtime::new()?.block_on(async_main(cli, config))
}

async fn async_main(cli: Cli, config: config::Config) -> anyhow::Result<()> {
    match cli.command {
        Commands::Daemon { passphrase } => {
            let passphrase = resolve_passphrase(passphrase)?;
            let key_path = config.key_path();

            let dek = if key_path.exists() {
                tracing::info!("unlocking existing encryption key...");
                storage::crypto::unlock(passphrase.as_bytes(), &key_path)?
            } else {
                tracing::info!("first run — setting up encryption key...");
                storage::crypto::setup_key(passphrase.as_bytes(), &key_path)?
            };

            let store = storage::Storage::open(&config, dek)?;
            daemon::run(config, store).await?;
        }

        Commands::Search { query, limit } => {
            cli::search(&config.grpc.listen_addr, &query, limit).await?;
        }

        Commands::Status => {
            cli::status(&config.grpc.listen_addr).await?;
        }

        Commands::Plugin { action } => {
            let addr = &config.grpc.listen_addr;
            match action {
                PluginAction::List => cli::plugin_list(addr).await?,
                PluginAction::Enable { name } => cli::plugin_enable(addr, &name).await?,
                PluginAction::Disable { name } => cli::plugin_disable(addr, &name).await?,
            }
        }
        Commands::Config { stdout, path } => {
            let action = if stdout {
                ConfigAction::Stdout
            } else if let Some(p) = path {
                ConfigAction::Path(p)
            } else {
                anyhow::bail!("either --stdout or a file path must be provided");
            };
            config.save()?;
        }
    }

    Ok(())
}

fn resolve_passphrase(arg: Option<String>) -> anyhow::Result<String> {
    if let Some(p) = arg {
        return Ok(p);
    }

    // Check env var (already handled by clap's `env`, but as fallback)
    if let Ok(p) = std::env::var("RECALLD_PASSPHRASE") {
        if !p.is_empty() {
            return Ok(p);
        }
    }

    // Prompt on stdin
    eprint!("Enter encryption passphrase: ");
    let mut passphrase = String::new();
    std::io::stdin().read_line(&mut passphrase)?;
    let passphrase = passphrase.trim().to_string();
    if passphrase.is_empty() {
        anyhow::bail!("passphrase cannot be empty");
    }
    Ok(passphrase)
}
