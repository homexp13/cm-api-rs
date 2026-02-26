use std::collections::HashMap;

use rocket::{State, http::Status, serde::json::Json};
use rocket::request::{FromRequest, Outcome};
use rocket_db_pools::Connection;
use serde::{Deserialize, Serialize};

use crate::{Cmdb, Config, token::create_token};

#[derive(Debug)]
pub struct ClientIp(pub Option<String>);

#[rocket::async_trait]
impl<'r> FromRequest<'r> for ClientIp {
    type Error = ();

    async fn from_request(req: &'r rocket::Request<'_>) -> Outcome<Self, Self::Error> {
        let ip = req.client_ip().map(|ip| ip.to_string());
        Outcome::Success(ClientIp(ip))
    }
}
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SteamConfig {
    pub web_api_key: String,
    pub app_id: HashMap<String, u32>,
}

#[derive(Debug, Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct SteamAuthRequest {
    pub ticket: String,
    pub steam_id: String,
    pub display_name: String,
    #[serde(default)]
    pub create_account_if_missing: bool,
    pub instance: String,
}

#[derive(Debug, Serialize)]
#[serde(crate = "rocket::serde")]
pub struct SteamAuthResponse {
    pub success: bool,
    pub user_exists: bool,
    pub access_token: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "rocket::serde")]
pub struct SteamErrorResponse {
    pub error: String,
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct SteamTicketResponse {
    response: SteamTicketResponseInner,
}

#[derive(Debug, Deserialize)]
struct SteamTicketResponseInner {
    params: Option<SteamTicketParams>,
    error: Option<SteamTicketError>,
}

#[derive(Debug, Deserialize)]
struct SteamTicketParams {
    result: String,
    #[serde(rename = "steamid")]
    steam_id: String,
    #[serde(rename = "ownersteamid")]
    #[allow(dead_code)]
    owner_steam_id: String,
    #[serde(rename = "vacbanned")]
    vac_banned: bool,
    #[serde(rename = "publisherbanned")]
    publisher_banned: bool,
}

#[derive(Debug, Deserialize)]
struct SteamTicketError {
    #[serde(rename = "errorcode")]
    error_code: i32,
    #[serde(rename = "errordesc")]
    error_desc: String,
}

/// Validates a Steam session ticket via the Steam Web API
async fn validate_steam_ticket(
    client: &reqwest::Client,
    config: &SteamConfig,
    app_to_use: &str,
    ticket: &str,
    expected_steam_id: &str,
) -> Result<SteamTicketParams, String> {
    let Some(id_to_use) = config.app_id.get(app_to_use) else {
        return Err("Incorrect App ID request.".to_string());
    };

    let url = format!(
        "https://api.steampowered.com/ISteamUserAuth/AuthenticateUserTicket/v1/?key={}&appid={}&ticket={}",
        config.web_api_key, id_to_use, ticket
    );

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Failed to contact Steam API: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Steam API returned error {}: {}", status, body));
    }

    let ticket_response: SteamTicketResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Steam API response: {}", e))?;

    if let Some(error) = ticket_response.response.error {
        return Err(format!(
            "Steam ticket validation failed ({}): {}",
            error.error_code, error.error_desc
        ));
    }

    let params = ticket_response
        .response
        .params
        .ok_or_else(|| "Steam API returned no params".to_string())?;

    if params.result != "OK" {
        return Err(format!("Steam ticket validation failed: {}", params.result));
    }

    // Verify the Steam ID matches what the client claimed
    if params.steam_id != expected_steam_id {
        return Err(format!(
            "Steam ID mismatch: expected {}, got {}",
            expected_steam_id, params.steam_id
        ));
    }

    // Check for bans
    if params.vac_banned {
        return Err("User is VAC banned".to_string());
    }

    if params.publisher_banned {
        return Err("User is publisher banned".to_string());
    }

    Ok(params)
}

/// POST /Steam/Authenticate - Authenticate a user via Steam
#[post("/Authenticate", format = "json", data = "<request>")]
pub async fn authenticate(
    config: &State<Config>,
    mut cmdb: Connection<Cmdb>,
    client_ip: ClientIp,
    request: Json<SteamAuthRequest>,
) -> Result<Json<SteamAuthResponse>, (Status, Json<SteamErrorResponse>)> {
    let steam_config = config.steam.as_ref().ok_or_else(|| {
        (
            Status::InternalServerError,
            Json(SteamErrorResponse {
                error: "not_configured".to_string(),
                message: "Steam authentication is not configured".to_string(),
            }),
        )
    })?;

    let http_client = reqwest::Client::new();

    // Validate the Steam ticket
    let _ticket_params = validate_steam_ticket(
        &http_client,
        steam_config,
        &request.instance,
        &request.ticket,
        &request.steam_id,
    )
    .await
    .map_err(|e| {
        (
            Status::Unauthorized,
            Json(SteamErrorResponse {
                error: "ticket_validation_failed".to_string(),
                message: e,
            }),
        )
    })?;
    let client_ip = client_ip.0.as_deref();
    let token = create_token(
        &mut cmdb,
        &request.steam_id,
        &request.display_name,
        client_ip,
        None,
    )
        .await
        .map_err(|e| {
            (
                Status::InternalServerError,
                Json(SteamErrorResponse {
                    error: "token_generation_failed".to_string(),
                    message: e,
                }),
            )
        })?;

    Ok(Json(SteamAuthResponse {
        success: true,
        user_exists: true,
        access_token: Some(token),
        error: None,
    }))
}

