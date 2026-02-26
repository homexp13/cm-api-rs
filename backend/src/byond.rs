use sqlx::{Error, prelude::FromRow};
use std::sync::Mutex;
use std::{io::Cursor, net::ToSocketAddrs};

use chrono::{DateTime, Duration, Utc};
use http2byond::ByondTopicValue;
use rocket::{
    Request, Response, State,
    http::{ContentType, Header, Status},
    response::{self, Responder},
    serde::json::Json,
};
use rocket_db_pools::Connection;
use sqlx::query_as;
use serde::Deserialize;

use crate::admin::AuthenticatedUser;
use crate::{Cmdb, Config, ServerConfig, admin::Staff};

/// sets `Access-Control-Allow-Origin: *` to allow requests from any origin.
pub struct PublicCors<T>(pub T);

impl<'r, T: serde::Serialize> Responder<'r, 'static> for PublicCors<T> {
    fn respond_to(self, _request: &'r Request<'_>) -> response::Result<'static> {
        let json = serde_json::to_string(&self.0).map_err(|_| Status::InternalServerError)?;

        Response::build()
            .header(ContentType::JSON)
            .header(Header::new("Access-Control-Allow-Origin", "*"))
            .header(Header::new("Access-Control-Allow-Methods", "GET, OPTIONS"))
            .header(Header::new("Access-Control-Allow-Headers", "*"))
            .sized_body(Some(json.len()), Cursor::new(json))
            .ok()
    }
}

#[derive(Default)]
pub struct ByondTopic {
    cached_status: Mutex<Option<ServersResponse>>,
    cache_time: Mutex<Option<DateTime<Utc>>>,
}

#[derive(serde::Serialize)]
struct GameRequest {
    query: String,
    auth: Option<String>,
    source: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct GameResponse {
    statuscode: i32,
    response: String,
    data: GameStatus,
}


fn deserialize_i32_from_bool_or_number<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(n) => n
            .as_i64()
            .and_then(|v| i32::try_from(v).ok())
            .ok_or_else(|| serde::de::Error::custom("invalid integer value")),
        serde_json::Value::Bool(b) => Ok(if b { 1 } else { 0 }),
        serde_json::Value::String(s) => s
            .parse::<i32>()
            .map_err(|_| serde::de::Error::custom("invalid numeric string")),
        _ => Err(serde::de::Error::custom(
            "expected number, bool, or numeric string",
        )),
    }
}
fn deserialize_i32_from_nullable_number<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(0),
        Some(serde_json::Value::Number(n)) => n
            .as_i64()
            .and_then(|v| i32::try_from(v).ok())
            .ok_or_else(|| serde::de::Error::custom("invalid integer value")),
        Some(serde_json::Value::Bool(b)) => Ok(if b { 1 } else { 0 }),
        Some(serde_json::Value::String(s)) => s
            .parse::<i32>()
            .map_err(|_| serde::de::Error::custom("invalid numeric string")),
        _ => Err(serde::de::Error::custom(
            "expected null, number, bool, or numeric string",
        )),
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(crate = "rocket::serde")]
pub struct GameStatus {
    mode: String,
    #[serde(deserialize_with = "deserialize_i32_from_bool_or_number")]
    vote: i32,
    #[serde(deserialize_with = "deserialize_i32_from_bool_or_number")]
    ai: i32,
    host: Option<String>,
    #[serde(deserialize_with = "deserialize_i32_from_nullable_number")]
    round_id: i32,
    players: i32,
    revision: String,
    admins: i32,
    gamestate: i32,
    map_name: String,
    security_level: String,
    round_duration: f32,
    time_dilation_current: f32,
    time_dilation_avg: f32,
    time_dilation_avg_slow: f32,
    time_dilation_avg_fast: f32,
    #[serde(default)]
    mcpu: Option<f32>,
    #[serde(default)]
    cpu: Option<f32>,
}

#[derive(serde::Serialize, Clone)]
pub struct ServerStatusResponse {
    name: String,
    url: String,
    status: String,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    details: Option<GameResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recommended_byond_version: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
}

#[derive(serde::Serialize, Clone)]
pub struct ServersResponse {
    servers: Vec<ServerStatusResponse>,
}

pub fn refresh_admins(config: &Config) -> Result<(), String> {
    let topic_config = config
        .topic
        .clone()
        .ok_or_else(|| "Topic config not available".to_string())?;

    for server in topic_config.servers.iter().filter(|s| s.refresh_admins) {
        let topic = format!("refresh_admins&key={}", server.auth);

        let addr = server
            .host
            .to_socket_addrs()
            .map_err(|e| format!("Failed to resolve host {}: {e}", server.name))?
            .next()
            .ok_or_else(|| format!("No socket address found for {}", server.name))?;

        if let Err(e) = http2byond::send_byond(&addr, &format!("?{topic}")) {
            eprintln!("Failed to send refresh_admins to {}: {e}", server.name);
        }
    }

    Ok(())
}

/// Fetches the current round information from all configured servers. **This is a public endpoint**.
#[get("/")]
pub async fn round(
    cache: &State<ByondTopic>,
    config: &State<Config>,
) -> Option<PublicCors<ServersResponse>> {
    {
        let cache_time_guard = match cache.cache_time.lock() {
            Ok(real) => real,
            Err(poisoned) => poisoned.into_inner(),
        };

        if let Some(cache_time) = *cache_time_guard {
            let cache_expiry = chrono::Utc::now() - Duration::seconds(20);
            if cache_time > cache_expiry {
                let cached = match cache.cached_status.lock() {
                    Ok(real) => real,
                    Err(poisoned) => poisoned.into_inner(),
                };
                if let Some(response) = cached.clone() {
                    return Some(PublicCors(response));
                }
            }
        }
    }

    let topic_config = config.topic.clone()?;

    if topic_config.servers.is_empty() {
        return None;
    }

    let handles: Vec<_> = topic_config
        .servers
        .into_iter()
        .map(|server| tokio::task::spawn_blocking(move || query_server(server)))
        .collect();

    let mut server_statuses = Vec::new();
    for handle in handles {
        if let Ok(status) = handle.await {
            server_statuses.push(status);
        }
    }

    let response = ServersResponse {
        servers: server_statuses,
    };

    *cache.cached_status.lock().unwrap() = Some(response.clone());
    *cache.cache_time.lock().unwrap() = Some(chrono::Utc::now());

    Some(PublicCors(response))
}

fn query_server(server: ServerConfig) -> ServerStatusResponse {
    let url = server.host.clone();
    let recommended_byond_version = server.recommended_byond_version.clone();
    let tags = server.tags.clone();

    let topic = format!("status&format=json&key={}", server.auth);

    let addr = match server.host.to_socket_addrs() {
        Ok(mut addrs) => match addrs.next() {
            Some(a) => a,
            None => {
                return ServerStatusResponse {
                    name: server.name,
                    url,
                    status: "unavailable".to_string(),
                    details: None,
                    recommended_byond_version,
                    tags,
                };
            }
        },
        Err(_) => {
            return ServerStatusResponse {
                name: server.name,
                url,
                status: "unavailable".to_string(),
                details: None,
                recommended_byond_version,
                tags,
            };
        }
    };

    let byond = match http2byond::send_byond(&addr, &format!("?{topic}")) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("send_byond failed for {} ({}): {}", server.name, url, e);
            return ServerStatusResponse {
                name: server.name,
                url,
                status: "unavailable".to_string(),
                details: None,
                recommended_byond_version,
                tags,
            };
        }
    };

    let byond_string = match byond {
        ByondTopicValue::String(s) => s,
        ByondTopicValue::Number(n) => {
            eprintln!("BYOND returned Number for {} ({}): {}", server.name, url, n);
            return ServerStatusResponse {
                name: server.name,
                url,
                status: "unavailable".to_string(),
                details: None,
                recommended_byond_version,
                tags,
            };
        }
        ByondTopicValue::None => {
            eprintln!("BYOND returned None for {} ({})", server.name, url);
            return ServerStatusResponse {
                name: server.name,
                url,
                status: "unavailable".to_string(),
                details: None,
                recommended_byond_version,
                tags,
            };
        }
    };

    let byond_string = byond_string.trim_end_matches('\0');

    match serde_json::from_str::<GameResponse>(byond_string) {
        Ok(game_response) => ServerStatusResponse {
            name: server.name,
            url,
            status: "available".to_string(),
            details: Some(game_response),
            recommended_byond_version,
            tags,
        },
        Err(_) => match serde_json::from_str::<GameStatus>(byond_string) {
            Ok(game_status) => ServerStatusResponse {
                name: server.name,
                url,
                status: "available".to_string(),
                details: Some(GameResponse {
                    statuscode: 200,
                    response: "ok".to_string(),
                    data: game_status,
                }),
                recommended_byond_version,
                tags,
            },
            Err(e) => {
                eprintln!("Failed to parse BYOND JSON for {} ({}): {}", server.name, url, e);
                eprintln!("Raw BYOND response: {}", byond_string);
                ServerStatusResponse {
                    name: server.name,
                    url,
                    status: "unavailable".to_string(),
                    details: None,
                    recommended_byond_version,
                    tags,
                }
            }
        },
    }
}

#[derive(serde::Serialize, FromRow)]
pub struct Round {
    id: i32,
}

#[get("/Recent")]
pub async fn recent(
    mut db: Connection<Cmdb>,
    _admin: AuthenticatedUser<Staff>,
) -> Json<Vec<Round>> {
    let rounds: Result<Vec<Round>, Error> =
        query_as("SELECT * FROM mc_round ORDER BY id DESC LIMIT ?")
            .bind(10)
            .fetch_all(&mut **db)
            .await;

    match rounds {
        Ok(rounds) => Json(rounds),
        Err(_) => Json(Vec::new()),
    }
}

#[derive(serde::Serialize)]
pub struct ByondHashResponse {
    sha256: Option<String>,
}

/// Returns the expected SHA256 hash for a given BYOND version. **This is a public endpoint**.
#[get("/?<byond_ver>")]
pub async fn byond_hash(config: &State<Config>, byond_ver: &str) -> PublicCors<ByondHashResponse> {
    let sha256 = config
        .byond_hashes
        .as_ref()
        .and_then(|hashes| hashes.get(byond_ver).cloned());

    PublicCors(ByondHashResponse { sha256 })
}






