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
use highland_control::{ControlRequest, ControlResponse, MAX_REQUEST_BYTES};
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
    /// Show the event history.
    Events {
        /// How many events to show at a time.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Keep asking for new events until interrupted.
        #[arg(long, short)]
        follow: bool,
        /// Return only events after this sequence number.
        #[arg(long, default_value_t = 0)]
        since: u64,
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
        Command::Events {
            limit,
            follow,
            since,
        } => {
            events(&cli.socket, *limit, *follow, *since, cli.json).await?;
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
        Command::Events {
            limit,
            follow,
            since,
        } => ControlRequest::Events {
            limit: Some(*limit),
            follow: *follow,
            since: Some(*since),
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

/// Prints the event history, and follows it by asking for what is new.
///
/// Following is a poll rather than a held-open connection on purpose: the
/// server must not keep a task per follower, because a client that disappeared
/// mid-stream would leave that task waiting on a socket forever. The cursor makes
/// each poll cheap, so a follower asks for a handful of new events rather than
/// re-reading the buffer.
async fn events(
    socket: &std::path::Path,
    limit: usize,
    follow: bool,
    since: u64,
    json: bool,
) -> anyhow::Result<()> {
    let mut cursor = since;

    loop {
        let request = ControlRequest::Events {
            limit: Some(limit),
            follow: true,
            since: Some(cursor),
        };
        let response = ask(socket, &request).await?;

        match response {
            ControlResponse::Events { events, latest } => {
                for event in events {
                    if json {
                        println!("{event}");
                    } else {
                        print_event(&event);
                    }
                }
                cursor = latest.max(cursor);
            }
            ControlResponse::Error { reason, message } => {
                anyhow::bail!("{reason}: {message}");
            }
            other @ ControlResponse::Ok { .. } => {
                print_human(&other);
            }
        }

        if !follow {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// Sends one request and returns the decoded response.
async fn ask(
    socket: &std::path::Path,
    request: &ControlRequest,
) -> anyhow::Result<ControlResponse> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .with_context(|| format!("could not reach the daemon at {}", socket.display()))?;
    let (reader, mut writer) = stream.into_split();

    let mut line = request.encode().context("encoding the control request")?;
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .await
        .context("writing the request")?;

    let mut reader = BufReader::new(reader);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .await
        .context("reading the response")?;
    ControlResponse::decode(response.trim_end()).context("decoding the response")
}

/// Renders one event for a human reader.
fn print_event(event: &serde_json::Value) {
    let role_change = if event.get("from").is_some() {
        format!("{} -> {}", field_of(event, "from"), field_of(event, "to"))
    } else {
        field_of(event, "reason")
    };
    println!(
        "{:>5}  {:<20} {:<12} {}",
        event
            .get("sequence")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        field_of(event, "name"),
        field_of(event, "instance"),
        role_change
    );
}

fn field_of(event: &serde_json::Value, name: &str) -> String {
    event
        .get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
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
        highland_control::ControlResponse::Events { events, .. } => {
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
