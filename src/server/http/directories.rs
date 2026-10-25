//! HTTP adapters; domain operations live in the application layer.
use crate::application::directory_queries;
use crate::application::directory_queries::{DirectoryListing, DirectoryQuery};
use crate::server::error::ApiError;
use axum::{Json, extract::Query};

pub async fn list(Query(query): Query<DirectoryQuery>) -> Result<Json<DirectoryListing>, ApiError> {
  Ok(Json(directory_queries::list(query).await?))
}
