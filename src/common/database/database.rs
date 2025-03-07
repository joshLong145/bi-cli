use super::models::{
    AiProvider, AnthropicConfig, OktaConfig, OneloginConfig, OpenaiConfig, Realm, Tenant, Token,
};

use crate::common::error::BiError;

use directories::ProjectDirs;
use log::debug;
use serde::{Deserialize, Serialize};
use sqlx::{
    migrate::{MigrateDatabase, Migrator},
    query, query_as,
    sqlite::SqlitePool,
    Row, Sqlite,
};

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

static MIGRATOR: Migrator = sqlx::migrate!();

const OKTA_CONFIG_KEY: &str = "okta_config";
const ONELOGIN_CONFIG_KEY: &str = "onelogin_config";
const OPENAI_CONFIG_KEY: &str = "openai_config";
const ANTHROPIC_CONFIG_KEY: &str = "anthropic_config";
const DEFAULT_AI_PROVIDER_KEY: &str = "default_ai_provider";

impl Database {
    // Initialize the database, create if not exists, and run migrations
    pub async fn initialize() -> Result<Self, BiError> {
        let db_url = Self::db_url()?;

        if !Sqlite::database_exists(&db_url).await.unwrap_or(false) {
            debug!("Creating database at {}", db_url);
            Sqlite::create_database(&db_url)
                .await
                .map_err(|e| BiError::StringError(e.to_string()))?;
        } else {
            debug!("Database already created at {}", db_url);
        }

        let pool = SqlitePool::connect(&db_url)
            .await
            .map_err(|e| BiError::StringError(e.to_string()))?;

        // Run migrations
        MIGRATOR
            .run(&pool)
            .await
            .map_err(|e| BiError::StringError(e.to_string()))?;

        for migration in MIGRATOR.migrations.iter() {
            debug!("Detected migration: {:?}", migration);
        }

        debug!("Database and migrations initialized successfully.");
        Ok(Database { pool })
    }

    // db_url creates and returns url of a database in a user writable
    // directory that is meant for storing application specific data
    fn db_url() -> Result<String, BiError> {
        let proj_dirs = ProjectDirs::from("com", "BeyondIdentity", env!("CARGO_PKG_NAME")).ok_or(
            BiError::StringError("Failed to determine project directory".to_string()),
        )?;
        let db_dir = proj_dirs.data_local_dir();
        std::fs::create_dir_all(db_dir).map_err(|e| BiError::StringError(e.to_string()))?;
        let db_path = db_dir.join("sqlite.db");
        let db_url = format!("sqlite://{}", db_path.display());
        Ok(db_url)
    }

    // Get all tenants with their corresponding realms
    pub async fn get_all_tenants_with_realms(&self) -> Result<Vec<(Tenant, Vec<Realm>)>, BiError> {
        // Fetch all tenants
        let tenants: Vec<Tenant> = query_as("SELECT * FROM tenants")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| BiError::StringError(e.to_string()))?;

        // For each tenant, fetch associated realms and construct TenantWithRealms
        let mut tenants_with_realms = Vec::new();
        for tenant in tenants {
            let realms: Vec<Realm> = query_as("SELECT * FROM realms WHERE tenant_id = ?")
                .bind(&tenant.id)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| BiError::StringError(e.to_string()))?;

            tenants_with_realms.push((tenant, realms));
        }
        Ok(tenants_with_realms)
    }

    // Set a new tenant and realm. Adds the tenant if it doesn't exist.
    pub async fn set_tenant_and_realm(&self, tenant: Tenant, realm: Realm) -> Result<(), BiError> {
        // Insert or ignore the tenant
        query("INSERT OR IGNORE INTO tenants (id) VALUES (?)")
            .bind(&tenant.id)
            .execute(&self.pool)
            .await
            .map_err(|e| BiError::StringError(e.to_string()))?;

        // Insert or replace the realm
        query("INSERT OR REPLACE INTO realms (id, tenant_id, application_id, client_id, client_secret, open_id_configuration_url, auth_base_url, api_base_url) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(&realm.id)
                .bind(&realm.tenant_id)
                .bind(&realm.application_id)
                .bind(&realm.client_id)
                .bind(&realm.client_secret)
                .bind(&realm.open_id_configuration_url)
                .bind(&realm.auth_base_url)
                .bind(&realm.api_base_url)
                .execute(&self.pool)
                .await
                .map_err(|e| BiError::StringError(e.to_string()))?;

        Ok(())
    }

    // Delete a tenant/realm pair, removing the tenant if it has no other realms.
    // Also unsets the default if the tenant/realm pair being deleted is set as the default.
    pub async fn delete_tenant_realm_pair(
        &self,
        tenant_id: &str,
        realm_id: &str,
    ) -> Result<(), BiError> {
        // Check if this tenant/realm pair is set as the default
        let is_default = query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM defaults WHERE tenant_id = ? AND realm_id = ?",
        )
        .bind(tenant_id)
        .bind(realm_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| BiError::StringError(e.to_string()))?
        .0 > 0;

        // First, delete the specific realm
        query("DELETE FROM realms WHERE tenant_id = ? AND id = ?")
            .bind(tenant_id)
            .bind(realm_id)
            .execute(&self.pool)
            .await
            .map_err(|e| BiError::StringError(e.to_string()))?;

        // Check if any realms remain for this tenant
        let remaining_realms_count: i64 =
            query_as::<_, (i64,)>("SELECT COUNT(*) FROM realms WHERE tenant_id = ?")
                .bind(tenant_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| BiError::StringError(e.to_string()))?
                .0;

        // If no realms remain, delete the tenant
        if remaining_realms_count == 0 {
            query("DELETE FROM tenants WHERE id = ?")
                .bind(tenant_id)
                .execute(&self.pool)
                .await
                .map_err(|e| BiError::StringError(e.to_string()))?;
        }

        // If this tenant/realm was set as the default, unset it
        if is_default {
            query("DELETE FROM defaults WHERE tenant_id = ? AND realm_id = ?")
                .bind(tenant_id)
                .bind(realm_id)
                .execute(&self.pool)
                .await
                .map_err(|e| BiError::StringError(e.to_string()))?;
        }

        Ok(())
    }

    // Get default tenant and realm
    pub async fn get_default_tenant_and_realm(&self) -> Result<Option<(Tenant, Realm)>, BiError> {
        // Fetch the default tenant and realm IDs
        if let Some(defaults) =
            query_as::<_, (String, String)>("SELECT tenant_id, realm_id FROM defaults WHERE id = 1")
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| BiError::StringError(e.to_string()))?
        {
            // Fetch the tenant by the default tenant_id
            let tenant = query_as::<_, Tenant>("SELECT * FROM tenants WHERE id = ?")
                .bind(&defaults.0)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| BiError::StringError(e.to_string()))?;

            // Fetch the realm by the default tenant_id and realm_id
            let realm = query_as::<_, Realm>("SELECT * FROM realms WHERE tenant_id = ? AND id = ?")
                .bind(&defaults.0)
                .bind(&defaults.1)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| BiError::StringError(e.to_string()))?;

            Ok(Some((tenant, realm)))
        } else {
            Ok(None)
        }
    }

    // Set default tenant and realm. There can only be one set at a time.
    pub async fn set_default_tenant_and_realm(
        &self,
        tenant_id: &str,
        realm_id: &str,
    ) -> Result<(), BiError> {
        // Insert or replace the default tenant and realm
        query("INSERT OR REPLACE INTO defaults (id, tenant_id, realm_id) VALUES (1, ?, ?)")
            .bind(tenant_id)
            .bind(realm_id)
            .execute(&self.pool)
            .await
            .map_err(|e| BiError::StringError(e.to_string()))?;
        Ok(())
    }

    // Get a token by tenant_id and realm_id
    pub async fn get_token(
        &self,
        tenant_id: &str,
        realm_id: &str,
    ) -> Result<Option<Token>, BiError> {
        let token =
            query_as::<_, Token>("SELECT * FROM tokens WHERE tenant_id = ? AND realm_id = ?")
                .bind(tenant_id)
                .bind(realm_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| BiError::StringError(e.to_string()))?;

        Ok(token)
    }

    // Set or update a token
    pub async fn set_token(&self, token: Token) -> Result<(), BiError> {
        query(
                "INSERT OR REPLACE INTO tokens (access_token, expires_at, tenant_id, realm_id, application_id)
                VALUES (?, ?, ?, ?, ?)"
            )
            .bind(&token.access_token)
            .bind(token.expires_at)
            .bind(&token.tenant_id)
            .bind(&token.realm_id)
            .bind(&token.application_id)
            .execute(&self.pool)
            .await
            .map_err(|e| BiError::StringError(e.to_string()))?;

        Ok(())
    }

    // Delete a token by tenant_id and realm_id
    pub async fn delete_token(&self, tenant_id: &str, realm_id: &str) -> Result<(), BiError> {
        query("DELETE FROM tokens WHERE tenant_id = ? AND realm_id = ?")
            .bind(tenant_id)
            .bind(realm_id)
            .execute(&self.pool)
            .await
            .map_err(|e| BiError::StringError(e.to_string()))?;

        Ok(())
    }

    // Get okta config from db
    pub async fn get_okta_config(&self) -> Result<Option<OktaConfig>, BiError> {
        self.get_config(OKTA_CONFIG_KEY).await
    }

    // Set okta config in db
    pub async fn set_okta_config(&self, config: OktaConfig) -> Result<(), BiError> {
        self.set_config(OKTA_CONFIG_KEY, &config).await
    }

    // Get onelogin config from db
    pub async fn get_onelogin_config(&self) -> Result<Option<OneloginConfig>, BiError> {
        self.get_config(ONELOGIN_CONFIG_KEY).await
    }

    // Set onelogin config in db
    pub async fn set_onelogin_config(&self, config: OneloginConfig) -> Result<(), BiError> {
        self.set_config(ONELOGIN_CONFIG_KEY, &config).await
    }

    // Get openai config from db
    pub async fn get_openai_config(&self) -> Result<Option<OpenaiConfig>, BiError> {
        self.get_config(OPENAI_CONFIG_KEY).await
    }

    // Set openai config in db
    pub async fn set_openai_config(&self, config: OpenaiConfig) -> Result<(), BiError> {
        self.set_config(OPENAI_CONFIG_KEY, &config).await
    }

    // Get anthropic config from db
    pub async fn get_anthropic_config(&self) -> Result<Option<AnthropicConfig>, BiError> {
        self.get_config(ANTHROPIC_CONFIG_KEY).await
    }

    // Set anthropic config in db
    pub async fn set_anthropic_config(&self, config: AnthropicConfig) -> Result<(), BiError> {
        self.set_config(ANTHROPIC_CONFIG_KEY, &config).await
    }

    // Get default AI provider
    pub async fn get_default_ai_provider(&self) -> Result<Option<AiProvider>, BiError> {
        self.get_config(DEFAULT_AI_PROVIDER_KEY).await
    }

    // Set default AI provider
    pub async fn set_default_ai_provider(&self, provider: AiProvider) -> Result<(), BiError> {
        self.set_config(DEFAULT_AI_PROVIDER_KEY, &provider).await
    }

    // Helper function to get a configuration from the settings table
    async fn get_config<T: for<'de> Deserialize<'de>>(
        &self,
        key: &str,
    ) -> Result<Option<T>, BiError> {
        let row = query("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| BiError::StringError(e.to_string()))?;

        if let Some(row) = row {
            let value: String = row
                .try_get("value")
                .map_err(|e| BiError::StringError(e.to_string()))?;
            let config: T =
                serde_json::from_str(&value).map_err(|e| BiError::StringError(e.to_string()))?;
            Ok(Some(config))
        } else {
            Ok(None)
        }
    }

    // Helper function to set a configuration in the settings table
    async fn set_config<T: Serialize>(&self, key: &str, config: &T) -> Result<(), BiError> {
        let value =
            serde_json::to_string(config).map_err(|e| BiError::StringError(e.to_string()))?;
        query("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)")
            .bind(key)
            .bind(value)
            .execute(&self.pool)
            .await
            .map_err(|e| BiError::StringError(e.to_string()))?;
        Ok(())
    }
}
