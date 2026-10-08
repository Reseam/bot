use std::env;

use anyhow::{Context, Result};

use super::*;

#[tokio::test]
#[ignore = "uploads to the live storage bucket"]
async fn live_share_round_trip_and_sweep() -> Result<()> {
    if let Err(error) = dotenvy::dotenv()
        && !error.not_found()
    {
        return Err(error).context("failed to load .env");
    }
    let storage = Storage::new(StorageConfig {
        endpoint: "https://sf-objectstorage.com".to_owned(),
        region: "ca".to_owned(),
        bucket: "bucket-722-1280".to_owned(),
        prefix: "reseam-bot/live-test/".to_owned(),
        access_key_id: env::var("STORAGE_ACCESS_KEY_ID")
            .context("STORAGE_ACCESS_KEY_ID is not set")?,
        secret_access_key: env::var("STORAGE_SECRET_ACCESS_KEY")
            .context("STORAGE_SECRET_ACCESS_KEY is not set")?,
    })?;
    let share = storage.share(0, "live test+1.txt");
    let http = Client::new();
    http.put(&share.upload)
        .body("shared")
        .send()
        .await?
        .error_for_status()?;
    let body = http
        .get(&share.download)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(body, "shared");
    assert_eq!(storage.sweep(LINK_TTL).await?, 0);
    assert_eq!(storage.sweep(Duration::ZERO).await?, 1);
    Ok(())
}
