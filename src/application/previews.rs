use crate::types::PreviewToken;
use anyhow::Result;
use chrono::Utc;
use serde::{Serialize, de::DeserializeOwned};
use sqlx::SqlitePool;

#[derive(Debug, Eq, PartialEq)]
pub enum PreviewError {
    AlreadyConsumed,
    Stale,
    NotUsable,
    Missing,
}

pub async fn invalidate(pool: &SqlitePool) -> Result<()> {
    sqlx::query("UPDATE previews SET status='stale' WHERE status='ready'")
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn store<T: Serialize>(
    pool: &SqlitePool,
    token: PreviewToken,
    items: &[T],
    summary: serde_json::Value,
    settings_fingerprint: String,
    duplicate_action: impl Fn(&T) -> Result<String>,
    track_id: impl Fn(&T) -> i64,
    candidate_id: impl Fn(&T) -> i64,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO previews(token,status,created_at,summary_json,settings_fingerprint) VALUES(?,'ready',?,?,?)",
    )
    .bind(token.to_string())
    .bind(&now)
    .bind(serde_json::to_string(&summary)?)
    .bind(settings_fingerprint)
    .execute(&mut *tx)
    .await?;
    for (position, item) in items.iter().enumerate() {
        sqlx::query(
            "INSERT INTO preview_items(preview_token,position,track_id,candidate_id,duplicate_action,item_json) VALUES(?,?,?,?,?,?)",
        )
        .bind(token.to_string())
        .bind(position as i64)
        .bind(track_id(item))
        .bind(candidate_id(item))
        .bind(duplicate_action(item)?)
        .bind(serde_json::to_string(item)?)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn consume<T: DeserializeOwned>(
    pool: &SqlitePool,
    token: PreviewToken,
) -> Result<Vec<T>, PreviewError> {
    let mut tx = pool.begin().await.map_err(|_| PreviewError::NotUsable)?;
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM previews WHERE token=?")
        .bind(token.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| PreviewError::NotUsable)?;
    match status.as_deref() {
        Some("ready") => {}
        Some("started" | "consumed") => return Err(PreviewError::AlreadyConsumed),
        Some("stale") => return Err(PreviewError::Stale),
        Some(_) => return Err(PreviewError::NotUsable),
        None => return Err(PreviewError::Missing),
    }
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT item_json FROM preview_items WHERE preview_token=? AND duplicate_action!='skip_duplicate' ORDER BY position",
    )
    .bind(token.to_string())
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| PreviewError::NotUsable)?;
    let items = rows
        .into_iter()
        .map(|row| serde_json::from_str(&row))
        .collect::<std::result::Result<Vec<T>, _>>()
        .map_err(|_| PreviewError::NotUsable)?;
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE previews SET status='started',started_at=?,consumed_at=? WHERE token=?")
        .bind(&now)
        .bind(&now)
        .bind(token.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|_| PreviewError::NotUsable)?;
    tx.commit().await.map_err(|_| PreviewError::NotUsable)?;
    Ok(items)
}

pub fn settings_fingerprint<T: Serialize>(settings: &T) -> Result<String> {
    Ok(serde_json::to_string(settings)?)
}
