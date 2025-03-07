use super::enrollment::{get_credentials_for_identity, Credential};
use super::resource_servers::fetch_beyond_identity_resource_servers;
use super::roles::fetch_role_memberships;

use crate::beyond_identity::api::common::api_client::ApiClient;
use crate::beyond_identity::api::common::service::IdentitiesService;
use crate::beyond_identity::api::identities::api::IdentitiesApi;
use crate::beyond_identity::api::identities::types::Identity;
use crate::common::error::BiError;

pub async fn delete_all_identities(api_client: &ApiClient) -> Result<(), BiError> {
    let (tenant, realm) = match api_client.db.get_default_tenant_and_realm().await? {
        Some((t, r)) => (t, r),
        None => {
            return Err(BiError::StringError(
                "No default tenant/realm set".to_string(),
            ))
        }
    };

    let mut url = format!(
        "{}/v1/tenants/{}/realms/{}/identities?page_size=200",
        realm.api_base_url, tenant.id, realm.id
    );

    loop {
        let response = api_client.client.get(&url).send().await?;

        let status = response.status();
        log::debug!("{} response status: {}", url, status);
        if !status.is_success() {
            let error_text = response.text().await?;
            return Err(BiError::RequestError(status, error_text));
        }

        let response_text = response.text().await?;
        log::debug!("{} response text: {}", url, response_text);
        let response_json: serde_json::Value = serde_json::from_str(&response_text)?;
        let page_identities: Vec<Identity> =
            serde_json::from_value(response_json["identities"].clone())?;

        for identity in &page_identities {
            IdentitiesService::new()
                .build()
                .await
                .delete_identity(&identity.id)
                .await
                .expect("Failed to delete identity");
            println!("Deleted identity {}", identity.id);
        }

        if let Some(next_page_token) = response_json
            .get("next_page_token")
            .and_then(|token| token.as_str())
        {
            url = format!(
                "{}/v1/tenants/{}/realms/{}/identities?page_size=200&page_token={}",
                realm.api_base_url, tenant.id, realm.id, next_page_token
            );
        } else {
            break;
        }
    }

    Ok(())
}

pub async fn delete_unenrolled_identities(api_client: &ApiClient) -> Result<(), BiError> {
    let (tenant, realm) = match api_client.db.get_default_tenant_and_realm().await? {
        Some((t, r)) => (t, r),
        None => {
            return Err(BiError::StringError(
                "No default tenant/realm set".to_string(),
            ))
        }
    };

    let mut url = format!(
        "{}/v1/tenants/{}/realms/{}/identities?page_size=200",
        realm.api_base_url, tenant.id, realm.id
    );

    loop {
        let response = api_client.client.get(&url).send().await?;

        let status = response.status();
        log::debug!("{} response status: {}", url, status);
        if !status.is_success() {
            let error_text = response.text().await?;
            return Err(BiError::RequestError(status, error_text));
        }

        let response_text = response.text().await?;
        log::debug!("{} response text: {}", url, response_text);
        let response_json: serde_json::Value = serde_json::from_str(&response_text)?;
        let page_identities: Vec<Identity> =
            serde_json::from_value(response_json["identities"].clone())?;

        for identity in &page_identities {
            let credentials = get_credentials_for_identity(api_client, &identity.id)
                .await
                .expect("Failed to fetch credentials");
            let enrolled = credentials
                .into_iter()
                .filter(|cred| cred.realm_id == realm.id && cred.tenant_id == tenant.id)
                .collect::<Vec<Credential>>();
            if enrolled.is_empty() {
                IdentitiesService::new()
                    .build()
                    .await
                    .delete_identity(&identity.id)
                    .await
                    .expect("Failed to delete identity");
                println!("Deleted identity {}", identity.id);
            }
        }

        if let Some(next_page_token) = response_json
            .get("next_page_token")
            .and_then(|token| token.as_str())
        {
            url = format!(
                "{}/v1/tenants/{}/realms/{}/identities?page_size=200&page_token={}",
                realm.api_base_url, tenant.id, realm.id, next_page_token
            );
        } else {
            break;
        }
    }

    Ok(())
}

pub async fn delete_norole_identities(api_client: &ApiClient) -> Result<(), BiError> {
    let (tenant, realm) = match api_client.db.get_default_tenant_and_realm().await? {
        Some((t, r)) => (t, r),
        None => {
            return Err(BiError::StringError(
                "No default tenant/realm set".to_string(),
            ))
        }
    };

    let mut url = format!(
        "{}/v1/tenants/{}/realms/{}/identities?page_size=200",
        realm.api_base_url, tenant.id, realm.id
    );

    let resource_servers = fetch_beyond_identity_resource_servers(api_client)
        .await
        .expect("Failed to fetch resource servers");

    loop {
        let response = api_client.client.get(&url).send().await?;

        let status = response.status();
        log::debug!("{} response status: {}", url, status);
        if !status.is_success() {
            let error_text = response.text().await?;
            return Err(BiError::RequestError(status, error_text));
        }

        let response_text = response.text().await?;
        log::debug!("{} response text: {}", url, response_text);
        let response_json: serde_json::Value = serde_json::from_str(&response_text)?;
        let page_identities: Vec<Identity> =
            serde_json::from_value(response_json["identities"].clone())?;

        for identity in &page_identities {
            let mut has_role = false;
            for resource_server in &resource_servers {
                let roles =
                    fetch_role_memberships(api_client, &identity.id, &resource_server.id).await?;

                has_role |= !roles.is_empty();
            }

            if !has_role {
                IdentitiesService::new()
                    .build()
                    .await
                    .delete_identity(&identity.id)
                    .await
                    .expect("Failed to delete identity");
                println!("Deleted identity {}", identity.id);
            }
        }

        if let Some(next_page_token) = response_json
            .get("next_page_token")
            .and_then(|token| token.as_str())
        {
            url = format!(
                "{}/v1/tenants/{}/realms/{}/identities?page_size=200&page_token={}",
                realm.api_base_url, tenant.id, realm.id, next_page_token
            );
        } else {
            break;
        }
    }

    Ok(())
}
