#![forbid(unsafe_code)]

use auth::{CorsConfig, OidcConfig};
use rocket::fairing::{Fairing, Info, Kind};
use rocket::figment::value::Value;
use rocket::fs::FileServer;
use rocket::http::Header;
use rocket::{Request, Response};
use rocket::{
    fairing::AdHoc,
    figment::{
        Figment,
        providers::{Format, Serialized, Toml},
    },
};
use rocket_db_pools::{Database, sqlx::MySqlPool};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::OidcClient;
use crate::authentik::AuthentikConfig;

#[macro_use]
extern crate rocket;

mod achievements;
mod admin;
mod auth;
mod authentik;
mod byond;
mod connections;
mod discord;
mod logging;
mod new_players;
mod player;
mod spa;
mod steam;
mod stickyban;
mod ticket;
mod token;
mod twofactor;
mod utils;
mod whitelist;

/// CORS fairing with configurable allowed origin
pub struct Cors {
    allowed_origin: String,
}

impl Cors {
    pub fn new(allowed_origin: String) -> Self {
        Self { allowed_origin }
    }
}

#[rocket::async_trait]
impl Fairing for Cors {
    fn info(&self) -> Info {
        Info {
            name: "Add CORS headers to responses",
            kind: Kind::Response,
        }
    }

    async fn on_response<'r>(&self, _request: &'r Request<'_>, response: &mut Response<'r>) {
        response.set_header(Header::new(
            "Access-Control-Allow-Origin",
            self.allowed_origin.clone(),
        ));
        response.set_header(Header::new(
            "Access-Control-Allow-Methods",
            "POST, GET, PATCH, DELETE, OPTIONS",
        ));
        response.set_header(Header::new("Access-Control-Allow-Headers", "*"));
        response.set_header(Header::new("Access-Control-Allow-Credentials", "true"));
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerConfig {
    pub name: String,
    pub host: String,
    pub auth: String,
    #[serde(default)]
    pub refresh_admins: bool,
    pub recommended_byond_version: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct TopicConfig {
    servers: Vec<ServerConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct LoggingConfig {
    webhook: String,
    user_manager_webhook: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ApiAuthConfig {
    /// Bearer token for API authorization (alternative to session-based auth)
    token: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct ServerRoleConfig {
    #[serde(default)]
    pub roles_to_add: Vec<String>,
    #[serde(default)]
    pub roles_to_remove: Vec<String>,
    /// minimum playtime in minutes required to be eligible for role changes.
    /// if the user has a database discord link, this requirement is bypassed.
    #[serde(default)]
    pub minimum_playtime_minutes: Option<i32>,
    /// mapping of whitelist status strings (e.g., "WHITELIST_SYNTHETIC") to role IDs.
    /// when a player has a whitelist_status containing one of these strings (separated by |),
    /// the corresponding roles will be added on link and removed on unlink.
    #[serde(default)]
    pub whitelist_roles: std::collections::HashMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DiscordBotConfig {
    pub token: String,
    /// mapping of server (guild) IDs to role configuration for link events
    /// on unlink events, the inverse is applied (roles_to_add are removed, roles_to_remove are added)
    #[serde(default)]
    pub link_role_changes: std::collections::HashMap<String, ServerRoleConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(crate = "rocket::serde")]
#[derive(Default)]
struct Config {
    topic: Option<TopicConfig>,
    logging: Option<LoggingConfig>,
    oidc: Option<OidcConfig>,
    cors: Option<CorsConfig>,
    authentik: Option<AuthentikConfig>,
    api_auth: Option<ApiAuthConfig>,
    discord_bot: Option<DiscordBotConfig>,
    steam: Option<steam::SteamConfig>,
    byond_hashes: Option<std::collections::HashMap<String, String>>,
}

#[derive(Database)]
#[database("cmdb")]
pub struct Cmdb(MySqlPool);

#[launch]
async fn rocket() -> _ {
    let figment = Figment::from(rocket::Config::default())
        .merge(Serialized::defaults(Config::default()))
        .merge(Toml::file("Rocket.toml").nested())
        .merge(Toml::file("Api.toml"));

    let base_url: String = match figment.find_value("host.base_url") {
        Ok(value) => match value {
            Value::String(_, val) => val,
            _ => panic!("base_url must be a string."),
        },
        Err(_) => "/api".to_string(),
    };

    // Extract configuration
    let config: Config = figment.extract().expect("Failed to extract configuration");

    // Get CORS allowed origin (default to * in debug mode)
    let allowed_origin = config
        .cors
        .as_ref()
        .map(|c| c.allowed_origin.clone())
        .unwrap_or_else(|| {
            if cfg!(debug_assertions) {
                "*".to_string()
            } else {
                panic!("CORS allowed_origin must be configured in Api.toml for production")
            }
        });

    // Initialize OIDC client if configured (not required in debug mode)
    let oidc_client = if let Some(oidc_config) = config.oidc.clone() {
        match auth::init_oidc_client(oidc_config).await {
            Ok(client) => Some(Arc::new(client)),
            Err(e) => {
                if cfg!(debug_assertions) {
                    eprintln!("Warning: Failed to initialize OIDC client: {}", e);
                    eprintln!("Continuing in debug mode without OIDC...");
                    None
                } else {
                    panic!("Failed to initialize OIDC client: {}", e);
                }
            }
        }
    } else if cfg!(debug_assertions) {
        eprintln!("Warning: OIDC not configured, running in debug mode");
        None
    } else {
        panic!("OIDC configuration required in Api.toml for production");
    };

    let mut rocket_builder = rocket::custom(figment)
        .manage(byond::ByondTopic::default())
        .attach(Cmdb::init())
        .attach(AdHoc::config::<Config>())
        .attach(Cors::new(allowed_origin));

    // Add OIDC client to managed state if available
    if let Some(client) = oidc_client {
        rocket_builder = rocket_builder.manage(client);
    } else {
        rocket_builder = rocket_builder.manage(Arc::new(OidcClient::default()))
    }

    rocket_builder
        .mount(
            format!("{base_url}/auth"),
            routes![
                auth::login,
                auth::callback,
                auth::logout,
                auth::refresh,
                auth::userinfo,
            ],
        )
        .mount(
            format!("{}/User", base_url),
            routes![
                player::index,
                player::id,
                player::new_note,
                player::applied_notes,
                player::get_playtime,
                player::get_recent_playtime,
                player::get_total_playtime,
                player::get_vpn_whitelist,
                player::add_vpn_whitelist,
                player::remove_vpn_whitelist,
                player::get_banned_players,
                player::get_ban_history,
                player::get_known_alts,
                player::add_known_alt,
                player::remove_known_alt,
            ],
        )
        .mount(
            format!("{}/Round", base_url),
            routes![byond::round, byond::recent],
        )
        .mount(
            format!("{}/Connections", base_url),
            routes![
                connections::ip,
                connections::cid,
                connections::ckey,
                connections::connection_history_by_cid,
                connections::connection_history_by_ip
            ],
        )
        .mount(
            format!("{}/Stickyban", base_url),
            routes![
                stickyban::all_stickybans,
                stickyban::whitelist,
                stickyban::get_matched_cids,
                stickyban::get_matched_ckey,
                stickyban::get_matched_ip,
                stickyban::get_all_cid,
                stickyban::get_all_ckey,
                stickyban::get_all_ip
            ],
        )
        .mount(
            format!("{}/Ticket", base_url),
            routes![ticket::get_tickets_by_round_id, ticket::get_tickets_by_user],
        )
        .mount(
            format!("{base_url}/Whitelist"),
            routes![whitelist::get_all_whitelistees],
        )
        .mount(
            format! {"{}/NewPlayers", base_url},
            routes![new_players::get_new_players],
        )
        .mount(
            format!("{}/TwoFactor", base_url),
            routes![twofactor::twofactor_validate],
        )
        .mount(
            format!("{}/Authentik", base_url),
            routes![
                authentik::add_user_to_group,
                authentik::remove_user_from_group,
                authentik::get_group_members,
                authentik::get_allowed_groups,
                authentik::get_allowed_instances,
                authentik::get_group_admin_ranks,
                authentik::update_group_admin_ranks,
                authentik::get_group_display_name,
                authentik::update_group_display_name,
                authentik::get_user_additional_titles,
                authentik::update_user_additional_titles,
                authentik::get_admin_ranks_export,
                authentik::get_discourse_user_id,
                authentik::webhook_user_unlinked,
                authentik::webhook_user_linked,
                authentik::get_user_by_uuid,
                authentik::get_user_by_discord_id,
                authentik::search_users,
                token::get_token_user_info,
            ],
        )
        .mount(
            format!("{}/Discord", base_url),
            routes![discord::get_user_by_discord, discord::check_verified],
        )
        .mount(format!("{}/Steam", base_url), routes![steam::authenticate])
        .mount(
            format!("{}/ByondHash", base_url),
            routes![byond::byond_hash],
        )
        .mount(
            format!("{}/Achievements", base_url),
            routes![
                achievements::get_achievements,
                achievements::set_achievement
            ],
        )
        .mount(
            "/",
            if std::path::Path::new("/var/www/static/assets").exists() {
                FileServer::from("/var/www/static/assets").into()
            } else {
                rocket::routes![]
            },
        )
        .mount("/", routes![spa::index, spa::fallback])
}
