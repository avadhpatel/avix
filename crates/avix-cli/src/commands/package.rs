use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::PathBuf;

use crate::util::expand_home;

#[derive(Subcommand)]
pub enum PackageCmd {
    /// Validate a package directory without building
    Validate {
        /// Path to the agent pack or service directory
        path: PathBuf,
    },
    /// Build a .tar.xz archive from a package directory
    Build {
        /// Path to the agent pack or service directory
        path: PathBuf,
        /// Output directory (default: current directory)
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
        /// Version string (e.g. v0.1.0)
        #[arg(long)]
        version: String,
        /// Skip pre-build validation
        #[arg(long)]
        skip_validation: bool,
    },
    /// Scaffold a new agent pack or service directory
    New {
        /// Package name
        name: String,
        /// Package type: agent or service
        #[arg(long = "type", value_parser = ["agent", "service"])]
        pkg_type: String,
        /// Initial version (default: 0.1.0)
        #[arg(long, default_value = "0.1.0")]
        version: String,
        /// Output directory (default: current directory)
        #[arg(long, short = 'o', default_value = ".")]
        output: PathBuf,
    },
    /// Manage trusted third-party signing keys
    Trust {
        #[command(subcommand)]
        sub: TrustCmd,
    },
}

#[derive(Subcommand)]
pub enum TrustCmd {
    /// Add a trusted signing key
    Add {
        /// Path to a local .asc key file, or https:// URL to fetch it from
        key: String,
        /// Human-readable label for this key (e.g. "AcmeCorp")
        #[arg(long)]
        name: String,
        /// Restrict this key to specific source patterns (e.g. "github:acmecorp/*")
        /// May be specified multiple times. Omit to trust for all sources.
        #[arg(long = "allow-source")]
        allow_sources: Vec<String>,
    },
    /// List all trusted keys
    List,
    /// Remove a trusted key by fingerprint
    Remove { fingerprint: String },
}

pub async fn run(sub: PackageCmd) -> Result<()> {
    use avix_core::packaging::{
        BuildRequest, PackageBuilder, PackageScaffold, PackageValidator, ScaffoldRequest,
    };

    match sub {
        PackageCmd::Validate { path } => match PackageValidator::validate(&path) {
            Ok(pkg_type) => {
                println!("✓ Valid {:?} package", pkg_type);
            }
            Err(errors) => {
                eprintln!("Validation failed ({} error(s)):", errors.len());
                for e in &errors {
                    eprintln!("  {}: {}", e.path, e.message);
                }
                std::process::exit(1);
            }
        },

        PackageCmd::Build {
            path,
            output,
            version,
            skip_validation,
        } => {
            let output_dir = output.unwrap_or_else(|| std::env::current_dir().unwrap());
            let req = BuildRequest {
                source_dir: path,
                output_dir,
                version,
                skip_validation,
            };
            let result = PackageBuilder::build(req).context("package build failed")?;
            println!("Built: {}", result.archive_path.display());
            println!("Checksum: {}", result.checksum_entry.trim());
        }

        PackageCmd::New {
            name,
            pkg_type,
            version,
            output,
        } => {
            let pkg_type = if pkg_type == "agent" {
                avix_core::packaging::PackageType::Agent
            } else {
                avix_core::packaging::PackageType::Service
            };
            let dir = PackageScaffold::create(ScaffoldRequest {
                name: name.clone(),
                pkg_type,
                version,
                output_dir: output,
            })
            .context("scaffold failed")?;
            println!("Created: {}", dir.display());
        }

        PackageCmd::Trust { sub } => {
            let root = expand_home(std::path::PathBuf::from(
                std::env::var("AVIX_ROOT").unwrap_or_else(|_| ".".to_string()),
            ));

            use avix_core::packaging::TrustStore;

            match sub {
                TrustCmd::Add {
                    key,
                    name,
                    allow_sources,
                } => {
                    let key_asc = if key.starts_with("https://") || key.starts_with("http://") {
                        let resp = reqwest::get(&key).await.context("fetch key from URL")?;
                        resp.text().await.context("read key response")?
                    } else {
                        std::fs::read_to_string(&key).context("read key file")?
                    };
                    let store = TrustStore::new(&root);
                    let trusted = store
                        .add(&key_asc, &name, allow_sources)
                        .context("add trusted key")?;
                    println!(
                        "Trusted key added: {} ({})",
                        trusted.label, trusted.fingerprint
                    );
                }
                TrustCmd::List => {
                    let store = TrustStore::new(&root);
                    let keys = store.list().context("list trusted keys")?;
                    if keys.is_empty() {
                        println!("No third-party keys trusted (official Avix key always active).");
                        return Ok(());
                    }
                    for k in &keys {
                        println!(
                            "{} — {} (added {})",
                            k.fingerprint,
                            k.label,
                            k.added_at.format("%Y-%m-%d"),
                        );
                        if k.allowed_sources.is_empty() {
                            println!("  allowed sources: all");
                        } else {
                            for s in &k.allowed_sources {
                                println!("  allowed source: {}", s);
                            }
                        }
                    }
                }
                TrustCmd::Remove { fingerprint } => {
                    let store = TrustStore::new(&root);
                    store.remove(&fingerprint).context("remove trusted key")?;
                    println!("Removed key: {}", fingerprint);
                }
            }
        }
    }

    Ok(())
}
