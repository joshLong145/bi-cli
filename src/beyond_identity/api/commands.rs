use async_trait::async_trait;
use clap::Subcommand;

use crate::beyond_identity::api::common::service::Service;
use crate::beyond_identity::tenant::load_tenant;
use crate::common::{command::Executable, config::Config, error::BiError};

use super::common::api_client::ApiClient;
use super::identities;

#[derive(Subcommand)]
pub enum BeyondIdentityApiCommands {
    /// Direct API calls for identities
    #[clap(subcommand)]
    Identities(identities::command::IdentityCommands),
}

#[async_trait]
impl Executable for BeyondIdentityApiCommands {
    async fn execute(&self) -> Result<(), BiError> {
        let config = Config::new();
        let tenant_config = load_tenant(&config).expect(
            "Failed to load tenant. Make sure you create a tenant before running this command.",
        );
        let api_client = ApiClient::new(&config, &tenant_config);
        match self {
            BeyondIdentityApiCommands::Identities(cmd) => {
                let result = cmd
                    .execute(&Service::new(api_client))
                    .await
                    .expect("Failed to execute identity command");
                println!("{}", result);
                Ok(())
            }
        }
    }
}
