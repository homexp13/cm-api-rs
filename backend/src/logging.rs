use std::error::Error;

use crate::Config;

#[derive(Debug)]
struct LogError;

impl std::fmt::Display for LogError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Logging not set up.")
    }
}

impl Error for LogError {}

#[derive(serde::Serialize)]
struct ExternalLogAuthor {
    name: String,
    url: String,
}

#[derive(serde::Serialize)]
struct ExternalLogEmbed {
    title: String,
    description: String,
    color: i32,
    author: ExternalLogAuthor,
}

#[derive(serde::Serialize)]
struct ExternalLog {
    embeds: Vec<ExternalLogEmbed>,
    username: String,
}

pub async fn log_external(
    config: &Config,
    title: String,
    message: String,
    use_user_manager_channel: bool,
) -> Result<(), Box<dyn Error>> {
    let public_url = option_env!("CM_API_PUBLIC_URL").unwrap_or("https://example.tgmc");

    let logging_config = match &config.logging {
        Some(logging) => logging,
        None => return Err(Box::new(LogError {})),
    };

    let webhook_url = if use_user_manager_channel {
        logging_config
            .user_manager_webhook
            .as_ref()
            .unwrap_or(&logging_config.webhook)
    } else {
        &logging_config.webhook
    };

    let json = ExternalLog {
        embeds: vec![ExternalLogEmbed {
            title,
            description: message,
            color: 8359053,
            author: ExternalLogAuthor {
                name: "[tgmc-db]".to_string(),
                url: public_url.to_string(),
            },
        }],
        username: "[tgmc-db]".to_string(),
    };

    match reqwest::Client::new()
        .post(webhook_url)
        .body(serde_json::to_string(&json)?)
        .header("Content-Type", "application/json")
        .send()
        .await
    {
        Ok(_) => Ok(()),
        Err(err) => panic!("{err:?}"),
    }
}
