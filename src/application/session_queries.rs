//! Read-only session views, context inspection and paged history lists.
use crate::application::app::App;
use crate::application::error::{ApplicationError, blocking};
pub use crate::storage::management::SessionQuery;
use crate::{
  session::SessionConfig,
  storage::{ReadList, StoredValue},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Page {
  pub start: u64,
  pub limit: usize,
}
impl Default for Page {
  fn default() -> Self {
    Self { start: 0, limit: 50 }
  }
}
async fn read_page<T: StoredValue + Serialize>(
  list: ReadList<T>,
  page: Page,
) -> Result<Value, ApplicationError> {
  blocking(move || {
    let page = list.read_page(page.start, page.limit)?;
    Ok(json!({"start":page.start,"items":page.items,"next":page.next}))
  })
  .await
}
pub async fn list(app: Arc<App>, query: SessionQuery) -> Result<Value, ApplicationError> {
  let management = app.management.clone();
  blocking(move || Ok(project_list(management.list(query)?))).await
}
pub async fn conversations(app: Arc<App>, query: SessionQuery) -> Result<Value, ApplicationError> {
  let management = app.management.clone();
  blocking(move || Ok(project_list(management.conversations(query)?))).await
}

pub async fn get(app: Arc<App>, id: String) -> Result<Value, ApplicationError> {
  Ok(app.get_session(&id).await?.describe())
}

pub async fn entries(app: Arc<App>, id: String, page: Page) -> Result<Value, ApplicationError> {
  read_page(app.get_session(&id).await?.reader.get_entries(), page).await
}
pub async fn queue(app: Arc<App>, id: String, page: Page) -> Result<Value, ApplicationError> {
  read_page(app.get_session(&id).await?.reader.get_message_queue(), page).await
}
pub async fn generations(app: Arc<App>, id: String, page: Page) -> Result<Value, ApplicationError> {
  read_page(app.get_session(&id).await?.reader.get_generations(), page).await
}
pub async fn generation_entries(
  app: Arc<App>,
  (id, generation): (String, u64),
  page: Page,
) -> Result<Value, ApplicationError> {
  let reader = app.get_session(&id).await?.reader.clone();
  let list = blocking(move || {
    let generation =
      reader.get_generations().get(generation)?.ok_or_else(ApplicationError::not_found)?;
    Ok(reader.get_generation_entry_ids(generation.id)?)
  })
  .await?;
  read_page(list, page).await
}
pub async fn calls(app: Arc<App>, id: String, page: Page) -> Result<Value, ApplicationError> {
  read_page(app.get_session(&id).await?.reader.get_model_calls(), page).await
}

pub async fn inspect_context(app: Arc<App>, id: String) -> Result<Value, ApplicationError> {
  let slot = app.get_session(&id).await?;
  let owner = slot.clone();
  let request = blocking(move || {
    let mut request = owner.sender.build_context_snapshot()?;
    request.tools = owner.configure_tools(SessionConfig::new(&request.model))?.tools;
    Ok(request)
  })
  .await?;
  let descriptor = slot.get_descriptor();
  let model = slot.make_model(app.get_provider(&descriptor.provider)?);
  let projected = model.inspect(&request).await.map_err(ApplicationError::internal)?;
  Ok(
    json!({"provider":descriptor.provider,"request":projected,"captured_at":crate::utils::time::Timestamp::now().0}),
  )
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StructurePage {
  start: u64,
  limit: usize,
  kind: Option<String>,
  origin: Option<String>,
}
impl Default for StructurePage {
  fn default() -> Self {
    Self { start: 0, limit: 20, kind: None, origin: None }
  }
}

pub async fn generation_structure(
  app: Arc<App>,
  (id, generation): (String, usize),
  page: StructurePage,
) -> Result<Value, ApplicationError> {
  if page.limit == 0 || page.limit > 100 {
    return Err(ApplicationError::invalid("limit must be between 1 and 100"));
  }
  if page.kind.as_deref().is_some_and(|value| {
    ![
      "System",
      "Developer",
      "User",
      "Assistant",
      "Reasoning",
      "ToolUse",
      "ToolResult",
      "UpstreamCompaction",
    ]
    .contains(&value)
  }) || page.origin.as_deref().is_some_and(|value| {
    !["Imported", "Input", "Model", "Tool", "Interrupted", "Context", "Summary"].contains(&value)
  }) {
    return Err(ApplicationError::invalid("unknown message type or origin"));
  }
  let reader = app.get_session(&id).await?.reader.clone();
  blocking(move || {
    if reader.get_generations().get(generation as u64)?.is_none() { return Err(ApplicationError::not_found()) }
    let (generation, count, positions, entries) = reader.generation_structure(crate::session::GenerationId(generation), page.start, page.limit, page.kind, page.origin)?;
    let mut items = Vec::new();
    for entry in entries.items {
      let mut value = json!(entry);
      let mut image_view = json!({"conversation":[value["message"].clone()]});
      crate::application::session_model::image_references(&mut image_view);
      value["message"] = image_view["conversation"][0].take();
      items.push(value);
    }
    Ok(json!({"generation":generation,"count":count,"positions":positions,"start":entries.start,"next":entries.next,"items":items}))
  }).await
}

fn project_list(mut page: Value) -> Value {
  if let Some(items) = page["items"].as_array_mut() {
    for record in items {
      crate::application::session::preview_pending_selection(record);
    }
  }
  page
}
