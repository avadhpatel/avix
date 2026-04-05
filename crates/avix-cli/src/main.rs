mod commands;
mod tui;
mod util;

pub use commands::*;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{filter::LevelFilter, layer::SubscriberExt};

use avix_client_core::persistence;

#[derive(Parser)]
#[command(name = "avix", about = "Avix agent OS", version)]
struct Cli {
    /// Output in JSON format
    #[arg(long)]
    json: bool,

    /// Log level
    #[arg(long = "log", default_value_t = LevelFilter::WARN, global = true)]
    log: LevelFilter,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Server-side: initialize, configure, and run the Avix runtime
    Server {
        #[command(subcommand)]
        sub: ServerCmd,
    },
    /// Client-side: connect to and interact with a running Avix server
    Client {
        #[command(subcommand)]
        sub: ClientCmd,
    },
    /// Build, validate, and scaffold Avix packages (offline — no server required)
    Package {
        #[command(subcommand)]
        sub: PackageCmd,
    },
}

fn log_filename(cmd: &Cmd) -> &str {
    match cmd {
        Cmd::Server { sub } => match sub {
            ServerCmd::Start { .. } => "server",
            ServerCmd::Run { .. } => "run",
            _ => "server",
        },
        Cmd::Client { sub } => match sub {
            ClientCmd::Tui { .. } => "tui",
            ClientCmd::Hil { .. } => "hil",
            ClientCmd::Logs { .. } => "logs",
            ClientCmd::Agent { .. } => "agent",
            ClientCmd::Service { .. } => "service",
            ClientCmd::Secret { .. } => "secret",
            ClientCmd::Session { .. } => "session",
            _ => "client",
        },
        Cmd::Package { .. } => "package",
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let log_level = cli.log;
    let log_dir = persistence::app_data_dir().join("logs");
    let log_filename = log_filename(&cli.command);
    let appender = RollingFileAppender::new(Rotation::DAILY, log_dir.clone(), log_filename);
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(appender)
                .with_target(false)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true)
                .json(),
        )
        .with(log_level);
    tracing::subscriber::set_global_default(subscriber)?;
    tracing::info!(
        "log_dir={} level={:?} filename={}",
        log_dir.display(),
        cli.log,
        log_filename
    );

    match cli.command {
        Cmd::Server { sub } => commands::server::run(sub).await?,
        Cmd::Client { sub } => commands::client::run(sub, cli.json).await?,
        Cmd::Package { sub } => commands::package::run(sub).await?,
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn client_connect_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "connect"]).unwrap();
        assert!(matches!(
            cli.command,
            Cmd::Client {
                sub: ClientCmd::Connect { .. }
            }
        ));
    }

    #[test]
    fn client_connect_with_config_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "connect",
            "--config",
            "/custom/config.yaml",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub: ClientCmd::Connect { config },
            } => assert_eq!(config, Some(PathBuf::from("/custom/config.yaml"))),
            _ => panic!("wrong variant"),
        }
    }

    // ── Service command tests ───────────────────────────────────────────────────

    #[test]
    fn service_install_parses_source_and_checksum() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "install",
            "./pkg.tar.gz",
            "--checksum",
            "sha256:abc123",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub:
                            ServiceCmd::Install {
                                source,
                                checksum,
                                no_verify,
                                ..
                            },
                    },
            } => {
                assert_eq!(source, "./pkg.tar.gz");
                assert_eq!(checksum.as_deref(), Some("sha256:abc123"));
                assert!(!no_verify);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_install_no_verify_flag() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "install",
            "./pkg.tar.gz",
            "--no-verify",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Install { no_verify, .. },
                    },
            } => assert!(no_verify),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_install_parses_root() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "install",
            "./pkg.tar.gz",
            "--root",
            "/custom/root",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Install { root, .. },
                    },
            } => assert_eq!(root.to_string_lossy(), "/custom/root"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn agent_install_parses_all_flags() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "agent",
            "install",
            "github:owner/repo/agent",
            "--scope",
            "system",
            "--version",
            "v1.0.0",
            "--checksum",
            "sha256:abc123",
            "--no-verify",
            "--session",
            "abc-123",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Agent {
                        sub:
                            AgentCmd::Install {
                                source,
                                scope,
                                version,
                                checksum,
                                no_verify,
                                session,
                                ..
                            },
                    },
            } => {
                assert_eq!(source, "github:owner/repo/agent");
                assert_eq!(scope, "system");
                assert_eq!(version.as_deref(), Some("v1.0.0"));
                assert_eq!(checksum.as_deref(), Some("sha256:abc123"));
                assert!(no_verify);
                assert_eq!(session.as_deref(), Some("abc-123"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn agent_install_dry_run_flag() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "agent",
            "install",
            "./agent.tar.xz",
            "--dry-run",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Agent {
                        sub: AgentCmd::Install { dry_run, .. },
                    },
            } => assert!(dry_run),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_list_subcommand_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "service", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Cmd::Client {
                sub: ClientCmd::Service {
                    sub: ServiceCmd::List { .. }
                }
            }
        ));
    }

    #[test]
    fn service_status_parses_name() {
        let cli =
            Cli::try_parse_from(["avix", "client", "service", "status", "github-svc"]).unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Status { name, .. },
                    },
            } => assert_eq!(name, "github-svc"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_start_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "service", "start", "my-svc"]).unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Start { name, server_url },
                    },
            } => {
                assert_eq!(name, "my-svc");
                assert_eq!(server_url, None);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_start_parses_server_url() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "start",
            "my-svc",
            "--server-url",
            "http://localhost:9999",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Start { name, server_url },
                    },
            } => {
                assert_eq!(name, "my-svc");
                assert_eq!(server_url, Some("http://localhost:9999".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_stop_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "service", "stop", "my-svc"]).unwrap();
        assert!(matches!(
            cli.command,
            Cmd::Client {
                sub: ClientCmd::Service {
                    sub: ServiceCmd::Stop { .. }
                }
            }
        ));
    }

    #[test]
    fn service_stop_parses_server_url() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "stop",
            "my-svc",
            "--server-url",
            "http://localhost:9999",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Stop { name, server_url },
                    },
            } => {
                assert_eq!(name, "my-svc");
                assert_eq!(server_url, Some("http://localhost:9999".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_restart_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "service", "restart", "my-svc"]).unwrap();
        assert!(matches!(
            cli.command,
            Cmd::Client {
                sub: ClientCmd::Service {
                    sub: ServiceCmd::Restart { .. }
                }
            }
        ));
    }

    #[test]
    fn service_restart_parses_server_url() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "restart",
            "my-svc",
            "--server-url",
            "http://localhost:9999",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Restart { name, server_url },
                    },
            } => {
                assert_eq!(name, "my-svc");
                assert_eq!(server_url, Some("http://localhost:9999".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_uninstall_force_flag() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "uninstall",
            "github-svc",
            "--force",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Uninstall { name, force, .. },
                    },
            } => {
                assert_eq!(name, "github-svc");
                assert!(force);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_logs_follow_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "logs",
            "github-svc",
            "--follow",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Logs { name, follow },
                    },
            } => {
                assert_eq!(name, "github-svc");
                assert!(follow);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_status_json_flag() {
        let cli = Cli::try_parse_from([
            "avix",
            "--json",
            "client",
            "service",
            "status",
            "github-svc",
        ])
        .unwrap();
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Cmd::Client {
                sub: ClientCmd::Service {
                    sub: ServiceCmd::Status { .. }
                }
            }
        ));
    }

    #[test]
    fn service_install_no_checksum_defaults_none() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "install",
            "https://example.com/x.tar.gz",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Install { checksum, .. },
                    },
            } => assert!(checksum.is_none()),
            _ => panic!("wrong variant"),
        }
    }

    // ── Secret command tests ──────────────────────────────────────────────────

    #[test]
    fn secret_set_for_service_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "secret",
            "set",
            "github-app-key",
            "ghp_abc",
            "--for-service",
            "github-svc",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Secret {
                        sub:
                            SecretCmd::Set {
                                name,
                                value,
                                for_service,
                                for_user,
                                ..
                            },
                    },
            } => {
                assert_eq!(name, "github-app-key");
                assert_eq!(value, "ghp_abc");
                assert_eq!(for_service.as_deref(), Some("github-svc"));
                assert!(for_user.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn secret_set_for_user_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "secret",
            "set",
            "my-token",
            "tok-xyz",
            "--for-user",
            "alice",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Secret {
                        sub:
                            SecretCmd::Set {
                                name,
                                value,
                                for_service,
                                for_user,
                                ..
                            },
                    },
            } => {
                assert_eq!(name, "my-token");
                assert_eq!(value, "tok-xyz");
                assert!(for_service.is_none());
                assert_eq!(for_user.as_deref(), Some("alice"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn secret_list_for_service_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "secret",
            "list",
            "--for-service",
            "github-svc",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Secret {
                        sub: SecretCmd::List { for_service, .. },
                    },
            } => assert_eq!(for_service.as_deref(), Some("github-svc")),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn secret_list_for_user_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "secret", "list", "--for-user", "alice"])
            .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Secret {
                        sub: SecretCmd::List { for_user, .. },
                    },
            } => assert_eq!(for_user.as_deref(), Some("alice")),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn secret_delete_for_service_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "secret",
            "delete",
            "my-key",
            "--for-service",
            "my-svc",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Secret {
                        sub:
                            SecretCmd::Delete {
                                name, for_service, ..
                            },
                    },
            } => {
                assert_eq!(name, "my-key");
                assert_eq!(for_service.as_deref(), Some("my-svc"));
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── Session command tests ──────────────────────────────────────────────────

    #[test]
    fn session_create_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "session",
            "create",
            "--title",
            "My Session",
            "--goal",
            "Do something",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Session {
                        sub: SessionCmd::Create { title, goal, .. },
                    },
            } => {
                assert_eq!(title, "My Session");
                assert_eq!(goal, "Do something");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn session_list_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "session", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Cmd::Client {
                sub: ClientCmd::Session {
                    sub: SessionCmd::List { .. }
                }
            }
        ));
    }

    #[test]
    fn session_show_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "session", "show", "sess-123"]).unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Session {
                        sub: SessionCmd::Show { session_id },
                    },
            } => assert_eq!(session_id, "sess-123"),
            _ => panic!("wrong variant"),
        }
    }

    // ── Package command tests (top-level) ─────────────────────────────────────

    #[test]
    fn package_validate_parses() {
        let cli = Cli::try_parse_from(["avix", "package", "validate", "/path/to/pkg"]).unwrap();
        match cli.command {
            Cmd::Package {
                sub: PackageCmd::Validate { path },
            } => assert_eq!(path, PathBuf::from("/path/to/pkg")),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn package_build_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "package",
            "build",
            "./agent-pack",
            "--output",
            "/out",
            "--version",
            "v1.0.0",
        ])
        .unwrap();
        match cli.command {
            Cmd::Package {
                sub:
                    PackageCmd::Build {
                        path,
                        output,
                        version,
                        skip_validation: _,
                    },
            } => {
                assert_eq!(path, PathBuf::from("./agent-pack"));
                assert_eq!(output, Some(PathBuf::from("/out")));
                assert_eq!(version, "v1.0.0");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn package_new_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "package",
            "new",
            "my-agent",
            "--type",
            "agent",
            "--version",
            "0.2.0",
            "--output",
            "/tmp/packages",
        ])
        .unwrap();
        match cli.command {
            Cmd::Package {
                sub:
                    PackageCmd::New {
                        name,
                        pkg_type,
                        version,
                        output,
                    },
            } => {
                assert_eq!(name, "my-agent");
                assert_eq!(pkg_type, "agent");
                assert_eq!(version, "0.2.0");
                assert_eq!(output, PathBuf::from("/tmp/packages"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn package_trust_add_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "package",
            "trust",
            "add",
            "--name",
            "Dev Key",
            "/keys/dev.asc",
            "--allow-source",
            "github.com/user/repo",
        ])
        .unwrap();
        match cli.command {
            Cmd::Package {
                sub:
                    PackageCmd::Trust {
                        sub:
                            TrustCmd::Add {
                                key,
                                name,
                                allow_sources,
                            },
                    },
            } => {
                assert_eq!(key, "/keys/dev.asc");
                assert_eq!(name, "Dev Key");
                assert_eq!(allow_sources, vec!["github.com/user/repo".to_string()]);
            }
            _ => panic!("wrong variant"),
        }
    }
}
