use crate::beyond_identity::api::common::{api_client::ApiClient, service::IdentitiesService};
use crate::beyond_identity::api::identities::api::IdentitiesApi;
use crate::beyond_identity::api::identities::types::Identity;
use crate::beyond_identity::helper::sso_configs;
use crate::common::database::models::OneloginConfig;
use crate::common::error::BiError;

use reqwest_middleware::ClientWithMiddleware as Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::{self, Write};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OneLoginApplication {
    id: u64,
    pub name: String,
    visible: bool,
    #[serde(default)]
    #[serde(rename = "users")]
    assigned_users: Vec<OneLoginUser>,
    #[serde(rename = "icon_url")]
    icon: Option<String>,
    #[serde(default)]
    #[serde(rename = "login_url")]
    login_link: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OneLoginUser {
    id: u64,
    email: Option<String>,
    username: Option<String>,
}

async fn get_onelogin_access_token(
    client: &Client,
    onelogin_config: &OneloginConfig,
) -> Result<String, BiError> {
    let url = format!("{}/auth/oauth2/v2/token", onelogin_config.domain);

    let payload = json!({
        "grant_type": "client_credentials",
        "client_id": onelogin_config.client_id,
        "client_secret": onelogin_config.client_secret,
    });

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    let status = response.status();
    let response_text = response.text().await?;

    log::debug!(
        "{} response status: {} and text: {}",
        url,
        status,
        response_text
    );

    if !status.is_success() {
        return Err(BiError::RequestError(status, response_text));
    }

    let response_json: serde_json::Value = serde_json::from_str(&response_text)?;
    let access_token = response_json
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| BiError::StringError("Access token not found".to_string()))?;

    Ok(access_token.to_string())
}

pub async fn fetch_onelogin_applications(
    client: &Client,
    onelogin_config: &OneloginConfig,
) -> Result<Vec<OneLoginApplication>, BiError> {
    let url = format!("{}/api/2/apps", onelogin_config.domain);

    let access_token = get_onelogin_access_token(client, onelogin_config).await?;

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer: {}", access_token))
        .send()
        .await?;

    let status = response.status();
    let response_text = response.text().await?;

    log::debug!(
        "{} response status: {} and text: {}",
        url,
        status,
        response_text
    );

    if !status.is_success() {
        return Err(BiError::RequestError(status, response_text));
    }

    let mut applications: Vec<OneLoginApplication> = serde_json::from_str(&response_text)?;

    for app in &mut applications {
        log::info!("Fetching assigned users for app: {:?}", app.name);
        let users = get_users_assigned_to_app(client, onelogin_config, app.id).await?;
        app.assigned_users = users;
        // Fetch application directly to get more info like "icon_url"
        let application =
            fetch_onelogin_application(client, onelogin_config, app.id, access_token.clone())
                .await?;
        app.icon = application.icon;
        app.login_link = format!("{}/launch/{}", onelogin_config.domain, app.id);
    }

    Ok(applications)
}

async fn fetch_onelogin_application(
    client: &Client,
    onelogin_config: &OneloginConfig,
    app_id: u64,
    access_token: String,
) -> Result<OneLoginApplication, BiError> {
    let url = format!("{}/api/2/apps/{}", onelogin_config.domain, app_id);

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer: {}", access_token))
        .send()
        .await?;

    let status = response.status();
    let response_text = response.text().await?;

    log::debug!(
        "{} response status: {} and text: {}",
        url,
        status,
        response_text
    );

    if !status.is_success() {
        return Err(BiError::RequestError(status, response_text));
    }

    let application: OneLoginApplication = serde_json::from_str(&response_text)?;

    Ok(application)
}

async fn get_users_assigned_to_app(
    client: &Client,
    onelogin_config: &OneloginConfig,
    app_id: u64,
) -> Result<Vec<OneLoginUser>, BiError> {
    let url = format!("{}/api/2/apps/{}/users", onelogin_config.domain, app_id);

    let access_token = get_onelogin_access_token(client, onelogin_config).await?;

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer: {}", access_token))
        .send()
        .await?;

    let status = response.status();
    let response_text = response.text().await?;

    log::debug!(
        "{} response status: {} and text: {}",
        url,
        status,
        response_text
    );

    if !status.is_success() {
        return Err(BiError::RequestError(status, response_text));
    }

    let assigned_users: Vec<OneLoginUser> = serde_json::from_str(&response_text)?;

    println!(
        "Fetched {} users for app id {}",
        assigned_users.len(),
        app_id
    );

    Ok(assigned_users)
}

pub fn select_applications(applications: &[OneLoginApplication]) -> Vec<OneLoginApplication> {
    println!("Select applications to fast migrate (comma separated indices or 'all' for all applications):");

    for (index, app) in applications.iter().enumerate() {
        println!(
            "{}: {} - {} (visible: {})",
            index, app.name, app.id, app.visible
        );
    }

    print!("Your selection: ");
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();

    if input == "all" {
        return applications.to_vec();
    }

    let indices: Vec<usize> = input
        .split(',')
        .map(|s| s.trim().parse().unwrap())
        .collect();

    indices
        .into_iter()
        .map(|i| applications[i].clone())
        .collect()
}

fn filter_identities(
    onelogin_users: &[OneLoginUser],
    beyond_identity_identities: &[Identity],
) -> Vec<Identity> {
    let onelogin_user_emails: Vec<&str> = onelogin_users
        .iter()
        .filter_map(|user| user.email.as_deref())
        .collect();
    beyond_identity_identities
        .iter()
        .filter(|identity| {
            identity
                .traits
                .primary_email_address
                .as_deref()
                .map_or(false, |email| onelogin_user_emails.contains(&email))
        })
        .cloned()
        .collect()
}

pub async fn create_sso_config_and_assign_identities(
    api_client: &ApiClient,
    onelogin_application: &OneLoginApplication,
) -> Result<sso_configs::SsoConfigBookmark, BiError> {
    let name = onelogin_application.name.clone();
    let login_link = onelogin_application.login_link.clone();
    let icon_url = onelogin_application.icon.clone();
    let sso_config =
        sso_configs::create_sso_config(&api_client, name, login_link, icon_url).await?;

    let beyond_identity_identities = IdentitiesService::new()
        .build()
        .await
        .list_identities(None, None)
        .await?
        .identities;
    let assigned_users = onelogin_application.assigned_users.as_ref();
    let filtered_identities = filter_identities(assigned_users, &beyond_identity_identities);

    sso_configs::assign_identities_to_sso_config(&api_client, &sso_config, &filtered_identities)
        .await?;

    Ok(sso_config)
}
