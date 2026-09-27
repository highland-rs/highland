// Rust guideline compliant 2026-09-27

//! The `highland` administrative command-line interface.
//!
//! The command set mirrors the control-API operation table in `SPEC.md` §22.1
//! exactly. A command that needs the daemon connects to the control socket; it
//! never reads the configuration file or talks to the kernel to answer a
//! runtime question (§22.2).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use highland_config::{ValidationContext, load_and_validate};
use highland_control::{ControlRequest, MAX_REQUEST_BYTES};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

mod run_daemon;

/// The default control socket path.
const DEFAULT_SOCKET: &str = "/run/highland/control.sock";

#[derive(Debug, Parser)]
#[command(
    name = "highland",
    version,
    about = "Administer the Highland VRRP failover daemon"
)]
struct Cli {
    /// The control socket to talk to.
    #[arg(long, global = true, default_value = DEFAULT_SOCKET)]
    socket: PathBuf,

    /// Print machine-readable JSON where the command supports it.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the daemon in the foreground.
    Run {
        /// The configuration file to load.
        #[arg(long, default_value = "/etc/highland/config.toml")]
        config: PathBuf,
        /// Accept a world-writable configuration file.
        #[arg(long)]
        allow_insecure_config: bool,
    },
    /// Parse and validate a configuration file, then exit.
    CheckConfig {
        /// The configuration file to check.
        config: PathBuf,
    },
    /// Show the status of every instance.
    Status,
    /// Show one instance in detail.
    Show {
        /// The instance name.
        instance: String,
    },
    /// Reload the configuration file.
    Reload {
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Ask a master instance to relinquish its VIPs.
    Relinquish {
        /// The instance name.
        instance: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Stop an instance from participating in election.
    Pause {
        /// The instance name.
        instance: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Resume a paused instance.
    Resume {
        /// The instance name.
        instance: String,
    },
    /// Force a role change. Requires the daemon to have been started with
    /// `--enable-force-transition`.
    ForceTransition {
        /// The instance name.
        instance: String,
        /// The role to force: init, backup, master, fault, or disabled.
        #[arg(long)]
        role: String,
        /// Required acknowledgement that this changes runtime state.
        #[arg(long)]
        enable: bool,
    },
    /// Stream events.
    Events {
        /// How many recent events to print first.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Keep streaming new events.
        #[arg(long, short)]
        follow: bool,
    },
    /// Print the version.
    Version,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("highland: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    match &cli.command {
        Command::Version => {
            println!("highland {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        Command::CheckConfig { config } => {
            check_config(config)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Run {
            config,
            allow_insecure_config,
        } => run_daemon::exec_daemon(config, *allow_insecure_config).map(|()| ExitCode::FAILURE),
        Command::ForceTransition {
            instance,
            role,
            enable,
        } => {
            // `R-10` and `R-29`: the CLI refuses before contacting the daemon,
            // so a mistyped command cannot half-apply.
            if !enable {
                anyhow::bail!(
                    "force-transition changes runtime state and must be acknowledged with --enable; \
                     target instance {instance:?} to role {role:?}"
                );
            }
            let request = ControlRequest::ForceTransition {
                instance: instance.clone(),
                role: role.clone(),
                confirm: true,
            };
            send(&cli.socket, request, cli.json).await?;
            Ok(ExitCode::SUCCESS)
        }
        other => {
            send(&cli.socket, request_for(other), cli.json).await?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn request_for(command: &Command) -> ControlRequest {
    match command {
        Command::Status => ControlRequest::Status,
        Command::Show { instance } => ControlRequest::Show {
            instance: instance.clone(),
        },
        Command::Reload { .. } => ControlRequest::Reload,
        Command::Relinquish { instance, .. } => ControlRequest::Relinquish {
            instance: instance.clone(),
        },
        Command::Pause { instance, .. } => ControlRequest::Pause {
            instance: instance.clone(),
        },
        Command::Resume { instance } => ControlRequest::Resume {
            instance: instance.clone(),
        },
        Command::Events { limit, follow } => ControlRequest::Events {
            limit: Some(*limit),
            follow: *follow,
        },
        Command::ForceTransition {
            instance,
            role,
            enable,
        } => ControlRequest::ForceTransition {
            instance: instance.clone(),
            role: role.clone(),
            confirm: *enable,
        },
        Command::Version | Command::CheckConfig { .. } | Command::Run { .. } => {
            unreachable!("the local commands never reach the control socket")
        }
    }
}

fn check_config(path: &std::path::Path) -> anyhow::Result<()> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let config = load_and_validate(&text, &ValidationContext::permissive())
        .with_context(|| format!("validating {}", path.display()))?;

    println!(
        "{} is valid: {} instance(s), schema version {}",
        path.display(),
        config.instances.len(),
        config.schema_version.0
    );
    Ok(())
}

async fn send(socket: &std::path::Path, request: ControlRequest, json: bool) -> anyhow::Result<()> {
    let line = request.encode().context("encoding the control request")?;
    anyhow::ensure!(
        line.len() <= MAX_REQUEST_BYTES,
        "request is {} bytes, above the {MAX_REQUEST_BYTES} byte limit",
        line.len()
    );

    let mut stream = tokio::net::UnixStream::connect(socket)
        .await
        .with_context(|| format!("could not reach the daemon at {}", socket.display()))?;

    stream
        .write_all(format!("{line}\n").as_bytes())
        .await
        .with_context(|| format!("writing to {}", socket.display()))?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .await
        .with_context(|| format!("reading from {}", socket.display()))?;

    let decoded = highland_control::ControlResponse::decode(response.trim_end())
        .context("the daemon sent a response this client cannot read")?;

    if json {
        println!("{}", response.trim_end());
    } else {
        print_human(&decoded);
    }

    // A refusal from the daemon is a failure of the command, and the exit code
    // has to say so: a script that runs `highland show nope && deploy` must not
    // go on to deploy.
    match &decoded {
        highland_control::ControlResponse::Error { reason, message } => {
            anyhow::bail!("{reason}: {message}")
        }
        _ => Ok(()),
    }
}

/// Renders a response for a human reader.
fn print_human(decoded: &highland_control::ControlResponse) {
    match decoded {
        highland_control::ControlResponse::Ok { status } => {
            println!("node {} (generation {})", status.node, status.generation);
            for instance in &status.instances {
                println!(
                    "  {:<16} {:<8} priority {} (effective {}) {}",
                    instance.name,
                    instance.role,
                    instance.priority,
                    instance.effective_priority,
                    if instance.vips_owned {
                        "[owning vips]"
                    } else {
                        ""
                    }
                );
            }
        }
        highland_control::ControlResponse::Events { events } => {
            for event in events {
                println!("{event}");
            }
        }
        highland_control::ControlResponse::Error { reason, message } => {
            // Printed as well as returned, so a human and a script see the same
            // refusal.
            println!("error [{reason}]: {message}");
        }
    }
}
