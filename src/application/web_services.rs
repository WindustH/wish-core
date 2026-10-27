//! Availability and connection checks for configured search and page-reading services.
use super::{app::App, error::ApplicationError};
use serde_json::{Value, json};

pub fn search_status(app: &App) -> Value {
  let accounts = app.get_service_accounts();
  json!({"providers":app.search.describe(&accounts),"available":app.search.is_available(&accounts)})
}
pub async fn check_search(app: &App, id: &str) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let accounts = app.get_service_accounts();
  app.lifecycle.until_shutdown(app.search.check(id, &accounts)).await?
}
pub fn fetch_status(app: &App) -> Value {
  app.fetch.describe()
}
pub async fn check_fetch(app: &App, id: &str) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let accounts = app.get_service_accounts();
  let page = app
    .lifecycle
    .until_shutdown(app.fetch.fetch(
      "https://example.com",
      false,
      &uuid::Uuid::new_v4().to_string(),
      &accounts,
      None,
      Some(id),
    ))
    .await?
    .map_err(ApplicationError::invalid)?;
  Ok(json!(page))
}
