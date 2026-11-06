//! Backlinks are derived from durable input metadata and completed runs, never browser state.
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
  source: String,
  cursor: Option<HistoryCursor>,
}
pub async fn read(app: Arc<App>, id: String, query: Query) -> Result<Value, ApplicationError> {
  if query.source.len() > 100
    || !query.source.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
  {
    return Err(ApplicationError::invalid("invalid reply source"));
  }
  let reader = app.get_session(&id).await?.reader.clone();
  blocking(move || {
    let (links, next) = reader.reply_links(query.source, query.cursor)?;
    Ok(json!({"items": links, "next": next}))
  })
  .await
}
