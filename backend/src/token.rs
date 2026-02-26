use rand::RngCore;
use rocket::{State, http::Status, serde::json::Json};
use rocket_db_pools::Connection;
use serde::Serialize;

use crate::{
    Cmdb, Config,
    authentik::{AuthentikConfig, AuthentikError, get_user_by_attribute},
    player::AuthorizationHeader,
};

const TOKEN_LIFETIME_HOURS: i64 = 3;

/// Generates a secure 64-character hex token (32 random bytes)
fn generate_secure_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Creates a new token for a steam_id and stores it in the database
pub async fn create_token(
    db: &mut Connection<Cmdb>,
    steam_id: &str,
    display_name: &str,
    ip: Option<&str>,
    cid: Option<&str>,
) -> Result<String, String> {
    let token = generate_secure_token();
    let expires_at = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::hours(TOKEN_LIFETIME_HOURS))
        .ok_or_else(|| "Failed to calculate expiration time".to_string())?;

    sqlx::query(
        r#"
        INSERT INTO steam_tokens (token, steam_id, display_name, ip, cid, expires_at)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&token)
    .bind(steam_id)
    .bind(display_name)
    .bind(ip)
    .bind(cid)
    .bind(expires_at)
    .execute(&mut ***db)
    .await
    .map_err(|e| format!("Failed to create token: {}", e))?;

    Ok(token)
}

/// Validates a token and returns the associated steam_id if valid and not expired
pub async fn validate_token(db: &mut Connection<Cmdb>, token: &str) -> Result<String, String> {
    let row: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT steam_id
        FROM steam_tokens
        WHERE token = ? AND expires_at > NOW()
        "#,
    )
    .bind(token)
    .fetch_optional(&mut ***db)
    .await
    .map_err(|e| format!("Failed to validate token: {}", e))?;

    match row {
        Some((steam_id,)) => Ok(steam_id),
        None => Err("Token is invalid or expired".to_string()),
    }
}

#[derive(Debug, Serialize)]
#[serde(crate = "rocket::serde")]
pub struct TokenUserInfoResponse {
    pub pk: i64,
    pub uid: String,
    pub uuid: String,
    pub name: String,
    pub username: String,
    pub attributes: serde_json::Value,
}

/// Validates a cm-api token and returns the user info from Authentik.
/// The token should be passed in the Authorization header as "Bearer <token>".
#[get("/TokenUserInfo")]
pub async fn get_token_user_info(
    auth_header: AuthorizationHeader,
    mut db: Connection<Cmdb>,
    config: &State<Config>,
) -> Result<Json<TokenUserInfoResponse>, (Status, Json<AuthentikError>)> {
    let authentik_config: &AuthentikConfig = config.authentik.as_ref().ok_or_else(|| {
        (
            Status::InternalServerError,
            Json(AuthentikError {
                error: "not_configured".to_string(),
                message: "Authentik is not configured".to_string(),
            }),
        )
    })?;

    // Extract the token from the Authorization header
    let token = auth_header.0.strip_prefix("Bearer ").ok_or_else(|| {
        (
            Status::Unauthorized,
            Json(AuthentikError {
                error: "invalid_authorization".to_string(),
                message: "Authorization header must be in format: Bearer <token>".to_string(),
            }),
        )
    })?;

    // Validate token in database and get steam_id
    let steam_id = validate_token(&mut db, token).await.map_err(|e| {
        (
            Status::Unauthorized,
            Json(AuthentikError {
                error: "token_invalid".to_string(),
                message: e,
            }),
        )
    })?;

    // Fetch user from Authentik by steam_id
    let http_client = reqwest::Client::new();
    let user = get_user_by_attribute(&http_client, authentik_config, "steam_id", &steam_id)
        .await
        .map_err(|e| {
            (
                Status::NotFound,
                Json(AuthentikError {
                    error: "user_not_found".to_string(),
                    message: format!("Failed to find user with steam_id '{}': {}", steam_id, e),
                }),
            )
        })?;

    Ok(Json(TokenUserInfoResponse {
        pk: user.pk,
        uid: user.uid,
        uuid: user.uuid.unwrap(),
        name: user.name,
        username: user.username,
        attributes: user.attributes,
    }))
}
