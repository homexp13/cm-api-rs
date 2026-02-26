# cm-api-rs

## Project Structure

This is a **monorepo** containing:

- `backend/` - Rust API server built with Rocket
- `frontend/` - React/TypeScript web interface

## Backend Configuration

The backend is configured via two TOML files: `Api.toml` and `Rocket.toml`.

### Api.toml

#### Host Configuration

```toml
[host]
base_url = "/api" # base path for API routes (default: "/api")
```

#### Topic Configuration

```toml
[topic]
host = "play.tgmc.example.com:1400" # game server address for status pings
auth = "your-comms-key"        # comms key to contact the game server
```

#### Logging Configuration

```toml
[logging]
webhook = "https://discord.com/api/webhooks/..."       # Discord webhook for database actions
user_manager_webhook = "https://discord.com/api/..."   # (optional) separate webhook for user management
```

#### API Authentication

```toml
[api_auth]
token = "your-bearer-token" # bearer token for API authorization (alternative to session-based auth)
```

#### CORS Configuration

```toml
[cors]
allowed_origin = "https://yourdomain.com" # allowed origin for CORS (required in production, defaults to "*" in debug)
```

#### OIDC Configuration

```toml
[oidc]
issuer_url = "https://auth.example.com/application/o/app/"
client_id = "your-client-id"
client_secret = "your-client-secret"
redirect_uri = "https://yourdomain.com/api/auth/callback"
scopes = ["openid", "profile", "email", "groups"]       # (optional) defaults to these values
staff_groups = ["staff", "admins"]                       # groups allowed to access the API
management_groups = ["management"]                       # groups with elevated permissions
session_secret = "your-secure-session-secret"            # secret for signing session JWTs
session_duration_hours = 24                              # (optional) session duration in hours (default: 24)
post_login_redirect = "/"                                # (optional) redirect after login (default: "/")
post_logout_redirect = "/"                               # (optional) redirect after logout (default: "/")
userinfo_endpoint = "https://auth.example.com/userinfo/" # (optional) override userinfo endpoint
```

#### Authentik Integration

```toml
[authentik]
token = "your-authentik-api-token"
base_url = "https://auth.example.com"
allowed_admin_ranks = ["R_ADMIN", "R_MOD", "R_EVENT"]    # allowed admin rank values for groups
allowed_instances = ["cm13-live", "cm13-rp"]             # allowed instance names for admin_ranks config
webhook_secret = "secret-for-webhook-auth"               # (optional) secret for authenticating webhooks

[authentik.group_permissions]
staff_management = ["admins", "moderators"]              # maps permission roles to manageable groups
mentor_overseer = ["mentors"]

[authentik.discourse]                                    # (optional) Discourse integration
base_url = "https://forum.example.com"
api_key = "your-discourse-api-key"
api_username = "system"
provider_name = "oidc"                                   # identity provider name in Discourse
webhook_secret = "discourse-webhook-secret"              # (optional) webhook authentication
```

#### Discord Bot Integration

```toml
[discord_bot]
token = "your-discord-bot-token"

[discord_bot.link_role_changes.123456789012345678]       # server (guild) ID
roles_to_add = ["987654321098765432"]                    # role IDs to add on link
roles_to_remove = ["111222333444555666"]                 # role IDs to remove on link
minimum_playtime_minutes = 60                            # (optional) minimum playtime required

[discord_bot.link_role_changes.123456789012345678.whitelist_roles]
WHITELIST_SYNTHETIC = ["synth-role-id"]                  # maps whitelist status to role IDs
WHITELIST_COMMANDER = ["co-role-id"]
```

### Rocket.toml

```toml
[default]
address = "0.0.0.0" # address to bind the web server (e.g., "0.0.0.0" for containers)
port = 8080         # port to listen on

[default.databases.cmdb]
url = "mysql://root:password@127.0.0.1:3306/tgmc_db" # MySQL connection string
```
