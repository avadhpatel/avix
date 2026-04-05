use anyhow::Context;
use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

use avix_core::secrets::SecretStore;

use crate::util::{emit, expand_home, owner_from};

#[derive(Subcommand)]
pub enum SecretCmd {
    /// Store a secret on disk (admin operation — requires AVIX_ROOT filesystem access)
    Set {
        /// Secret name
        name: String,
        /// Secret value
        value: String,
        /// Store for a service (mutually exclusive with --for-user)
        #[arg(long = "for-service", conflicts_with = "for_user")]
        for_service: Option<String>,
        /// Store for a user (mutually exclusive with --for-service)
        #[arg(long = "for-user", conflicts_with = "for_service")]
        for_user: Option<String>,
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: PathBuf,
    },
    /// List secret names for an owner
    List {
        /// List secrets for a service
        #[arg(long = "for-service", conflicts_with = "for_user")]
        for_service: Option<String>,
        /// List secrets for a user
        #[arg(long = "for-user", conflicts_with = "for_service")]
        for_user: Option<String>,
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: PathBuf,
    },
    /// Delete a secret from disk
    Delete {
        /// Secret name
        name: String,
        /// Delete from service owner
        #[arg(long = "for-service", conflicts_with = "for_user")]
        for_service: Option<String>,
        /// Delete from user owner
        #[arg(long = "for-user", conflicts_with = "for_service")]
        for_user: Option<String>,
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: PathBuf,
    },
}

pub async fn run(sub: SecretCmd, json: bool) -> Result<()> {
    let master_key: [u8; 32] = {
        let raw = std::env::var("AVIX_MASTER_KEY").context("AVIX_MASTER_KEY is not set")?;
        let bytes = raw.as_bytes();
        let mut key = [0u8; 32];
        let len = bytes.len().min(32);
        key[..len].copy_from_slice(&bytes[..len]);
        key
    };

    match sub {
        SecretCmd::Set {
            name,
            value,
            for_service,
            for_user,
            root,
        } => {
            let root = expand_home(root);
            let owner = owner_from(for_service, for_user)?;
            let store = SecretStore::new(&root.join("secrets"), &master_key);
            store
                .set(&owner, &name, &value)
                .context("failed to set secret")?;
            emit(
                json,
                |_: &()| format!("Secret '{name}' set for {owner}"),
                (),
            );
        }

        SecretCmd::List {
            for_service,
            for_user,
            root,
        } => {
            let root = expand_home(root);
            let owner = owner_from(for_service, for_user)?;
            let store = SecretStore::new(&root.join("secrets"), &master_key);
            let names = store.list(&owner);
            emit(
                json,
                |names: &Vec<String>| {
                    if names.is_empty() {
                        format!("No secrets for {owner}")
                    } else {
                        names.join("\n")
                    }
                },
                names,
            );
        }

        SecretCmd::Delete {
            name,
            for_service,
            for_user,
            root,
        } => {
            let root = expand_home(root);
            let owner = owner_from(for_service, for_user)?;
            let store = SecretStore::new(&root.join("secrets"), &master_key);
            store
                .delete(&owner, &name)
                .context("failed to delete secret")?;
            emit(
                json,
                |_: &()| format!("Secret '{name}' deleted for {owner}"),
                (),
            );
        }
    }

    Ok(())
}
