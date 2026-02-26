use std::collections::HashMap;

use chrono::TimeZone;
use rocket::{
    Request, State,
    form::Form,
    futures::TryStreamExt,
    http::Status,
    request::{FromRequest, Outcome},
    serde::{Deserialize, Serialize, json::Json},
};
use rocket_db_pools::Connection;
use sqlx::{MySqlConnection, Row, prelude::FromRow, query, query_as, types::BigDecimal};

use crate::{
    Cmdb, Config,
    admin::{AuthenticatedUser, Player as PlayerPermission, Staff},
    logging::log_external,
};

/// Request guard for extracting the Authorization header
pub struct AuthorizationHeader(pub String);

#[rocket::async_trait]
impl<'r> FromRequest<'r> for AuthorizationHeader {
    type Error = ();

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        match request.headers().get_one("Authorization") {
            Some(value) => Outcome::Success(AuthorizationHeader(value.to_string())),
            None => Outcome::Forward(Status::Unauthorized),
        }
    }
}

/// Validates the Authorization header against the configured API auth token.
/// Returns true if the header is valid, false otherwise.
pub fn validate_auth_header(auth_header: Option<&str>, config: &Config) -> bool {
    let Some(auth_value) = auth_header else {
        return false;
    };

    let Some(api_auth) = &config.api_auth else {
        return false;
    };

    // Support both "Bearer <token>" and raw "<token>" formats
    let token = auth_value.strip_prefix("Bearer ").unwrap_or(auth_value);

    token == api_auth.token
}

#[derive(Serialize, FromRow)]
#[serde(crate = "rocket::serde", rename_all = "camelCase")]
pub struct Player {
    id: i64,
    ckey: Option<String>,
    last_login: Option<String>,
    is_permabanned: Option<i32>,
    permaban_reason: Option<String>,
    permaban_date: Option<String>,
    permaban_admin_id: Option<i64>,
    is_time_banned: Option<i32>,
    time_ban_reason: Option<String>,
    time_ban_admin_id: Option<i64>,
    time_ban_date: Option<String>,
    time_ban_expiration: Option<i64>,
    last_known_ip: Option<String>,
    last_known_cid: Option<String>,
    migrated_notes: Option<i32>,
    migrated_bans: Option<i32>,
    migrated_jobbans: Option<i32>,
    stickyban_whitelisted: Option<i32>,
    whitelist_status: Option<String>,
    byond_account_age: Option<String>,
    first_join_date: Option<String>,

    #[sqlx(skip)]
    notes: Vec<Note>,

    #[sqlx(skip)]
    job_bans: Vec<JobBan>,

    #[sqlx(skip)]
    permaban_admin_ckey: Option<String>,

    #[sqlx(skip)]
    time_ban_admin_ckey: Option<String>,

    #[sqlx(skip)]
    discord_id: Option<String>,
}

impl Player {
    async fn add_metadata(mut self, db: &mut MySqlConnection) -> Player {
        if let Some(notes) = get_player_notes(db, self.id).await {
            self.notes = notes
        }

        if let Some(jobbans) = get_player_jobbans(db, self.id).await {
            self.job_bans = jobbans
        }

        if self.is_permabanned.is_some()
            && self.is_permabanned.unwrap() != 0
            && self.permaban_admin_id.is_some()
        {
            self.permaban_admin_ckey = get_player_ckey(db, self.permaban_admin_id.unwrap()).await;
        }

        if self.is_time_banned.is_some()
            && self.is_time_banned.unwrap() != 0
            && self.time_ban_admin_id.is_some()
        {
            self.time_ban_admin_ckey = get_player_ckey(db, self.time_ban_admin_id.unwrap()).await;
        }

        self.discord_id = get_discord_id_from_player_id(db, self.id).await;

        self
    }
}

#[derive(Serialize, FromRow)]
#[serde(crate = "rocket::serde", rename_all = "camelCase")]
pub struct Note {
    id: i64,
    player_id: i64,
    admin_id: i64,
    text: Option<String>,
    date: String,
    is_ban: i32,
    ban_time: Option<i64>,
    is_confidential: i32,
    admin_rank: String,
    note_category: Option<i32>,
    round_id: Option<i32>,

    #[sqlx(skip)]
    noted_player_ckey: Option<String>,

    #[sqlx(skip)]
    noting_admin_ckey: Option<String>,
}

async fn get_player_notes(db: &mut MySqlConnection, id: i64) -> Option<Vec<Note>> {
    let mut user_notes: Vec<Note> = match query_as("SELECT * FROM player_notes WHERE player_id = ?")
        .bind(id)
        .fetch_all(&mut *db)
        .await
    {
        Ok(notes) => notes,
        Err(err) => panic!("{}", err),
    };

    for note in &mut user_notes {
        note.noting_admin_ckey = get_player_ckey(db, note.admin_id).await;
        note.noted_player_ckey = get_player_ckey(db, note.player_id).await;
    }

    Some(user_notes)
}

#[get("/<id>/AppliedNotes")]
pub async fn applied_notes(
    mut db: Connection<Cmdb>,
    _admin: AuthenticatedUser<Staff>,
    id: i64,
) -> Json<Vec<Note>> {
    let mut user_notes: Vec<Note> = match query_as("SELECT * FROM player_notes WHERE admin_id = ?")
        .bind(id)
        .fetch_all(&mut **db)
        .await
    {
        Ok(notes) => notes,
        Err(_) => return Json(Vec::new()),
    };

    for note in &mut user_notes {
        note.noting_admin_ckey = get_player_ckey(&mut db, note.admin_id).await;
        note.noted_player_ckey = get_player_ckey(&mut db, note.player_id).await;
    }

    Json(user_notes)
}

#[derive(Serialize, FromRow)]
#[serde(crate = "rocket::serde", rename_all = "camelCase")]
pub struct JobBan {
    id: i64,
    player_id: i64,
    admin_id: Option<i64>,
    text: String,
    date: Option<String>,
    ban_time: Option<i64>,
    expiration: Option<i64>,
    role: String,

    #[sqlx(skip)]
    banning_admin_ckey: Option<String>,
}

async fn get_player_jobbans(db: &mut MySqlConnection, id: i64) -> Option<Vec<JobBan>> {
    let mut jobbans: Vec<JobBan> =
        match query_as("SELECT * FROM player_job_bans WHERE player_id = ?")
            .bind(id)
            .fetch_all(&mut *db)
            .await
        {
            Ok(jobbans) => jobbans,
            Err(err) => panic!("{err:?}"),
        };

    for jobban in &mut jobbans.iter_mut() {
        if jobban.admin_id.is_some() {
            jobban.banning_admin_ckey = get_player_ckey(&mut *db, jobban.admin_id.unwrap()).await;
        }
    }

    Some(jobbans)
}

async fn get_discord_id_from_player_id(db: &mut MySqlConnection, id: i64) -> Option<String> {
    match query("SELECT discord_id FROM discord_links WHERE player_id = ?")
        .bind(id)
        .fetch_one(db)
        .await
    {
        Ok(search) => Some(search.get("discord_id")),
        Err(_) => None,
    }
}

#[get("/?<ckey>&<discord_id>")]
pub async fn index(
    mut db: Connection<Cmdb>,
    admin: Option<AuthenticatedUser<Staff>>,
    config: &State<Config>,
    auth_header: Option<AuthorizationHeader>,
    ckey: Option<String>,
    discord_id: Option<String>,
) -> Result<Json<Player>, Status> {
    // Check if authorized via Staff guard or Authorization header
    let is_authorized = admin.is_some()
        || validate_auth_header(auth_header.as_ref().map(|h| h.0.as_str()), config.inner());

    if !is_authorized {
        return Err(Status::Unauthorized);
    }
    let user_result: Result<Player, sqlx::Error>;

    if let Some(ckey) = ckey {
        user_result = query_as("SELECT * FROM players WHERE ckey = ?")
            .bind(ckey)
            .fetch_one(&mut **db)
            .await;
    } else if let Some(discord_id) = discord_id {
        let player_id: i64 = match query("SELECT player_id FROM discord_links WHERE discord_id = ?")
            .bind(discord_id)
            .fetch_one(&mut **db)
            .await
        {
            Ok(search) => search.get("player_id"),
            Err(_) => return Err(Status::NotFound),
        };

        user_result = query_as("SELECT * FROM players WHERE id = ?")
            .bind(player_id)
            .fetch_one(&mut **db)
            .await;
    } else {
        return Err(Status::BadRequest);
    }

    let user = match user_result {
        Ok(user) => user,
        Err(_) => return Err(Status::NotFound),
    };

    Ok(Json(user.add_metadata(&mut db).await))
}

#[get("/<id>")]
pub async fn id(
    mut db: Connection<Cmdb>,
    _admin: AuthenticatedUser<Staff>,
    id: i32,
) -> Option<Json<Player>> {
    let user: Player = match query_as("SELECT * FROM players WHERE id = ?")
        .bind(id)
        .fetch_one(&mut **db)
        .await
    {
        Ok(user) => user,
        Err(error) => panic!("Error retrieving data: {error:?}"),
    };

    Some(Json(user.add_metadata(&mut db).await))
}

#[derive(FromForm)]
pub struct NewNote {
    message: String,
    category: i32,
    confidential: bool,
}

#[post("/<id>/Note", data = "<input>")]
pub async fn new_note(
    mut db: Connection<Cmdb>,
    admin: AuthenticatedUser<Staff>,
    id: i64,
    input: Form<NewNote>,
    config: &State<Config>,
) -> Status {
    let admin_id = match get_player_id(&mut db, &admin.ckey).await {
        Some(admin_id) => admin_id,
        None => return Status::Unauthorized,
    };

    let ckey = match get_player_ckey(&mut db, id).await {
        Some(ckey) => ckey,
        None => return Status::BadRequest,
    };

    match create_note(
        &mut db,
        id,
        admin_id,
        &input.message,
        input.confidential,
        input.category,
    )
    .await
    {
        true => {
            let _ = log_external(
                config,
                "Note Added".to_string(),
                format!(
                    "{} added a note to {}: {}",
                    &admin.ckey, ckey, &input.message
                ),
                false,
            )
            .await;
            Status::Accepted
        }
        false => Status::NotAcceptable,
    }
}

pub async fn create_note(
    db: &mut MySqlConnection,
    id: i64,
    admin_id: i64,
    message: &String,
    confidential: bool,
    category: i32,
) -> bool {
    match query("
    INSERT INTO player_notes (player_id, admin_id, text, date, is_ban, is_confidential, admin_rank, note_category)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
    .bind(id)
    .bind(admin_id)
    .bind(message)
    .bind(chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string())
    .bind(0)
    .bind(if confidential {1} else {0})
    .bind("[tgmc-db]".to_string())
    .bind(category)
    .execute(db).await {
        Ok(query) => query.rows_affected() > 0,
        Err(_) => false,
    }
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Playtime {
    id: i64,
    player_id: i64,
    role_id: String,
    total_minutes: i32,
}

#[get("/<id>/Playtime")]
pub async fn get_playtime(
    mut db: Connection<Cmdb>,
    _admin: AuthenticatedUser<Staff>,
    id: i64,
) -> Json<Vec<Playtime>> {
    match query_as("SELECT * FROM player_playtime WHERE player_id = ?")
        .bind(id)
        .fetch_all(&mut **db)
        .await
    {
        Ok(some) => Json(some),
        Err(_) => Json(Vec::new()),
    }
}

pub async fn query_total_playtime_minutes(db: &mut Connection<Cmdb>, ckey: &str) -> Option<i32> {
    match query(r"SELECT SUM(player_playtime.total_minutes) AS playtime FROM players INNER JOIN player_playtime ON players.id = player_playtime.player_id WHERE players.ckey = ? AND player_playtime.role_id != 'Observer'")
        .bind(ckey)
        .fetch_one(&mut ***db)
        .await
    {
        Ok(row) => {
            if let Ok(decimal) = row.try_get::<BigDecimal, _>("playtime") {
                use std::str::FromStr;
                i32::from_str(&decimal.to_string()).ok()
            } else {
                Some(0)
            }
        }
        Err(_) => Some(0),
    }
}

#[get("/TotalPlaytime?<ckey>")]
pub async fn get_total_playtime(
    mut db: Connection<Cmdb>,
    admin: Option<AuthenticatedUser<Staff>>,
    config: &State<Config>,
    auth_header: Option<AuthorizationHeader>,
    ckey: String,
) -> Result<Json<String>, Status> {
    // Check if authorized via Staff guard or Authorization header
    let is_authorized = admin.is_some()
        || validate_auth_header(auth_header.as_ref().map(|h| h.0.as_str()), config.inner());

    if !is_authorized {
        return Err(Status::Unauthorized);
    }

    match query_total_playtime_minutes(&mut db, &ckey).await {
        Some(minutes) => Ok(Json(minutes.to_string())),
        None => Ok(Json("0".to_string())),
    }
}

#[get("/<id>/Playtime/<days>")]
pub async fn get_recent_playtime(
    mut db: Connection<Cmdb>,
    _admin: AuthenticatedUser<Staff>,
    id: i64,
    days: i64,
) -> Json<Vec<Playtime>> {
    let time_since = chrono::Utc
        .with_ymd_and_hms(2000, 1, 1, 0, 0, 0)
        .unwrap()
        .timestamp_millis();

    let time_ago = chrono::Utc::now() - chrono::Duration::days(days);
    let time_millis = time_ago.timestamp_millis();
    let time_since = (time_millis - time_since) / 100;

    let mut rows =
        query("SELECT * FROM log_player_playtime WHERE player_id = ? AND real_time_recorded > ?")
            .bind(id)
            .bind(time_since)
            .fetch(&mut **db);

    let mut playtimes: HashMap<String, i64> = HashMap::new();

    let mut row_result = rows.try_next().await;
    while row_result.is_ok() {
        match row_result.as_mut().unwrap() {
            Some(value) => {
                let role: String = value.get("role_id");
                let mut deciseconds: i64 = value.get("total_deciseconds");

                if let Some(existing) = playtimes.get(&role) {
                    deciseconds += existing;
                }

                playtimes.insert(role, deciseconds);
            }
            None => break,
        };

        row_result = rows.try_next().await;
    }

    let mut playtime_structs = Vec::new();

    for playtime in playtimes.iter().enumerate() {
        let minutes = playtime.1.1 / 600;

        playtime_structs.push(Playtime {
            id: playtime.0 as i64,
            player_id: id,
            role_id: playtime.1.0.to_string(),
            total_minutes: minutes as i32,
        });
    }

    Json(playtime_structs)
}

pub async fn get_player_id(db: &mut MySqlConnection, ckey: &String) -> Option<i64> {
    let player_id: i64 = match query("SELECT id FROM players WHERE ckey = ?")
        .bind(ckey)
        .fetch_one(db)
        .await
    {
        Ok(search) => search.get("id"),
        Err(_) => return None,
    };

    Some(player_id)
}

pub async fn get_player_ckey(db: &mut MySqlConnection, id: i64) -> Option<String> {
    let player_ckey: String = match query("SELECT ckey FROM players WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
    {
        Ok(search) => search.get("ckey"),
        Err(_) => return None,
    };

    Some(player_ckey)
}

#[post("/VpnWhitelist?<ckey>")]
pub async fn add_vpn_whitelist(
    mut db: Connection<Cmdb>,
    admin: AuthenticatedUser<Staff>,
    ckey: String,
) -> Status {
    match query("INSERT INTO vpn_whitelist (ckey, admin_ckey) VALUES (?, ?)")
        .bind(&ckey)
        .bind(&admin.ckey)
        .execute(&mut **db)
        .await
    {
        Ok(_) => Status::Created,
        Err(_) => Status::InternalServerError,
    }
}

#[delete("/VpnWhitelist?<ckey>")]
pub async fn remove_vpn_whitelist(
    mut db: Connection<Cmdb>,
    _admin: AuthenticatedUser<Staff>,
    ckey: String,
) -> Status {
    match query("DELETE FROM vpn_whitelist WHERE ckey = ?")
        .bind(&ckey)
        .execute(&mut **db)
        .await
    {
        Ok(_) => Status::Ok,
        Err(_) => Status::InternalServerError,
    }
}

#[derive(Serialize, FromRow)]
#[serde(crate = "rocket::serde", rename_all = "camelCase")]
pub struct VpnWhitelist {
    ckey: String,
    admin_ckey: String,
}

#[get("/VpnWhitelist?<ckey>")]
pub async fn get_vpn_whitelist(
    mut db: Connection<Cmdb>,
    _admin: AuthenticatedUser<Staff>,
    ckey: String,
) -> Option<Json<VpnWhitelist>> {
    match query_as("SELECT ckey, admin_ckey FROM vpn_whitelist WHERE ckey = ?")
        .bind(&ckey)
        .fetch_one(&mut **db)
        .await
    {
        Ok(whitelist) => Some(Json(whitelist)),
        Err(_) => None,
    }
}

#[derive(Serialize, FromRow)]
#[serde(crate = "rocket::serde", rename_all = "camelCase")]
pub struct BannedPlayer {
    ckey: Option<String>,
    is_time_banned: Option<i32>,
    time_ban_reason: Option<String>,
    time_ban_date: Option<String>,
    time_ban_expiration: Option<i64>,
    is_permabanned: Option<i32>,
    permaban_reason: Option<String>,
    permaban_date: Option<String>,
}

/// Returns all currently banned players (permabanned or active time bans)
/// Accessible by any authenticated user
/// Supports pagination with ?page=N (0-indexed, 20 results per page)
/// Supports filtering by ckey with ?ckey=<search>
#[get("/Banned?<page>&<ckey>")]
pub async fn get_banned_players(
    mut db: Connection<Cmdb>,
    _user: AuthenticatedUser<PlayerPermission>,
    page: Option<i64>,
    ckey: Option<String>,
) -> Json<Vec<BannedPlayer>> {
    let byond_epoch = chrono::Utc
        .with_ymd_and_hms(2000, 1, 1, 0, 0, 0)
        .unwrap()
        .timestamp();
    let current_byond_time = (chrono::Utc::now().timestamp() - byond_epoch) / 60;

    let page = page.unwrap_or(0).max(0);
    let offset = page * 20;

    let cutoff_date = "2026-02-10";

    let result = if let Some(ckey_filter) = ckey {
        let ckey_pattern = format!("%{}%", ckey_filter);
        query_as(
            r"SELECT ckey, is_time_banned, time_ban_reason, time_ban_date, time_ban_expiration,
                     is_permabanned, permaban_reason, permaban_date
              FROM players
              WHERE ckey LIKE ?
                AND ((is_permabanned = 1 AND permaban_reason IS NOT NULL AND permaban_date LIKE '202%' AND permaban_date >= ?)
                   OR (is_time_banned = 1 AND time_ban_date IS NOT NULL AND time_ban_date LIKE '202%' AND time_ban_date >= ? AND time_ban_expiration > ?))
              ORDER BY COALESCE(permaban_date, time_ban_date) DESC
              LIMIT 20 OFFSET ?",
        )
        .bind(ckey_pattern)
        .bind(cutoff_date)
        .bind(cutoff_date)
        .bind(current_byond_time)
        .bind(offset)
        .fetch_all(&mut **db)
        .await
    } else {
        query_as(
            r"SELECT ckey, is_time_banned, time_ban_reason, time_ban_date, time_ban_expiration,
                     is_permabanned, permaban_reason, permaban_date
              FROM players
              WHERE (is_permabanned = 1 AND permaban_reason IS NOT NULL AND permaban_date LIKE '202%' AND permaban_date >= ?)
                 OR (is_time_banned = 1 AND time_ban_date IS NOT NULL AND time_ban_date LIKE '202%' AND time_ban_date >= ? AND time_ban_expiration > ?)
              ORDER BY COALESCE(permaban_date, time_ban_date) DESC
              LIMIT 20 OFFSET ?",
        )
        .bind(cutoff_date)
        .bind(cutoff_date)
        .bind(current_byond_time)
        .bind(offset)
        .fetch_all(&mut **db)
        .await
    };

    match result {
        Ok(players) => Json(players),
        Err(_) => Json(Vec::new()),
    }
}

#[derive(Serialize, FromRow)]
#[serde(crate = "rocket::serde", rename_all = "camelCase")]
pub struct HistoricalBan {
    ckey: Option<String>,
    text: Option<String>,
    date: String,
    ban_time: Option<i64>,
    round_id: Option<i32>,
}

/// Returns historical bans from player notes (is_ban = 1)
/// Accessible by any authenticated user
/// Supports pagination with ?page=N (0-indexed, 20 results per page)
/// Supports filtering by ckey with ?ckey=<search>
#[get("/BanHistory?<page>&<ckey>")]
pub async fn get_ban_history(
    mut db: Connection<Cmdb>,
    _user: AuthenticatedUser<PlayerPermission>,
    page: Option<i64>,
    ckey: Option<String>,
) -> Json<Vec<HistoricalBan>> {
    let page = page.unwrap_or(0).max(0);
    let offset = page * 20;

    let cutoff_date = "2026-02-10";

    let result = if let Some(ckey_filter) = ckey {
        let ckey_pattern = format!("%{}%", ckey_filter);
        query_as(
            r"SELECT p.ckey, n.text, n.date, n.ban_time, n.round_id
              FROM player_notes n
              INNER JOIN players p ON n.player_id = p.id
              WHERE n.is_ban = 1 AND p.ckey LIKE ? AND n.date LIKE '202%' AND n.date >= ?
              ORDER BY n.id DESC
              LIMIT 20 OFFSET ?",
        )
        .bind(ckey_pattern)
        .bind(cutoff_date)
        .bind(offset)
        .fetch_all(&mut **db)
        .await
    } else {
        query_as(
            r"SELECT p.ckey, n.text, n.date, n.ban_time, n.round_id
              FROM player_notes n
              INNER JOIN players p ON n.player_id = p.id
              WHERE n.is_ban = 1 AND n.date LIKE '202%' AND n.date >= ?
              ORDER BY n.id DESC
              LIMIT 20 OFFSET ?",
        )
        .bind(cutoff_date)
        .bind(offset)
        .fetch_all(&mut **db)
        .await
    };

    match result {
        Ok(bans) => Json(bans),
        Err(_) => Json(Vec::new()),
    }
}

#[derive(Serialize)]
#[serde(crate = "rocket::serde", rename_all = "camelCase")]
pub struct KnownAltsResponse {
    main_account: Option<String>,
    alts: Vec<String>,
}

/// Returns known alts for a given ckey
/// Checks if the ckey is a main account (player_ckey) or an alt account (ckey)
#[get("/KnownAlts?<ckey>")]
pub async fn get_known_alts(
    mut db: Connection<Cmdb>,
    _admin: AuthenticatedUser<Staff>,
    ckey: String,
) -> Json<KnownAltsResponse> {
    let main_account: Option<String> =
        query("SELECT player_ckey FROM known_alts WHERE ckey = ? LIMIT 1")
            .bind(&ckey)
            .fetch_optional(&mut **db)
            .await
            .ok()
            .flatten()
            .map(|row| row.get("player_ckey"));

    let lookup_ckey = main_account.as_ref().unwrap_or(&ckey);

    let alts: Vec<String> = match query("SELECT ckey FROM known_alts WHERE player_ckey = ?")
        .bind(lookup_ckey)
        .fetch_all(&mut **db)
        .await
    {
        Ok(rows) => rows
            .iter()
            .filter_map(|row| row.get::<Option<String>, _>("ckey"))
            .filter(|alt| alt != &ckey)
            .collect(),
        Err(_) => Vec::new(),
    };

    Json(KnownAltsResponse {
        main_account: if main_account.is_some() {
            main_account
        } else {
            None
        },
        alts,
    })
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde", rename_all = "camelCase")]
pub struct AddKnownAltRequest {
    player_ckey: String,
    alt_ckey: String,
}

/// Adds a known alt association
#[post("/KnownAlts", data = "<request>")]
pub async fn add_known_alt(
    mut db: Connection<Cmdb>,
    _admin: AuthenticatedUser<Staff>,
    request: rocket::serde::json::Json<AddKnownAltRequest>,
) -> Status {
    let result = query(
        "INSERT INTO known_alts (player_ckey, ckey) VALUES (?, ?)",
    )
    .bind(&request.player_ckey)
    .bind(&request.alt_ckey)
    .execute(&mut **db)
    .await;

    match result {
        Ok(_) => Status::Created,
        Err(_) => Status::InternalServerError,
    }
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde", rename_all = "camelCase")]
pub struct RemoveKnownAltRequest {
    player_ckey: String,
    alt_ckey: String,
}

/// Removes a known alt association
#[delete("/KnownAlts", data = "<request>")]
pub async fn remove_known_alt(
    mut db: Connection<Cmdb>,
    _admin: AuthenticatedUser<Staff>,
    request: rocket::serde::json::Json<RemoveKnownAltRequest>,
) -> Status {
    let result = query(
        "DELETE FROM known_alts WHERE player_ckey = ? AND ckey = ?",
    )
    .bind(&request.player_ckey)
    .bind(&request.alt_ckey)
    .execute(&mut **db)
    .await;

    match result {
        Ok(_) => Status::Ok,
        Err(_) => Status::InternalServerError,
    }
}
