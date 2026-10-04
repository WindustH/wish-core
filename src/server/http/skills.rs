//! Skills over HTTP: what `wish skill` asks through the bridge, and the settings page's view of
//! them. A session's skills switch is enforced here and nowhere else.
use super::bridge;
use crate::server::{
  app::App,
  config::SkillsConfig,
  error::{ApiError, blocking},
  skills::{self, Root, Skill},
};
use axum::{
  Json,
  extract::{Path, Query, State},
  http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// Where the session a bridge request speaks for finds skills, when its skills switch is on.
async fn session_roots(
  app: &Arc<App>,
  id: &str,
  headers: &HeaderMap,
) -> Result<(Vec<Root>, SkillsConfig), ApiError> {
  let slot = bridge::session(app, id, headers).await?;
  // Said to the model through the command's output, so it knows why and who can change it.
  let descriptor = slot.get_descriptor();
  if !descriptor.tools.skills {
    return Err(ApiError::conflict(
      "Skills are disabled for this session. The user can enable them in the session's settings.",
    ));
  }
  let config = app.config_file.lock().await.config.skills.clone();
  Ok((skills::roots(Some(&descriptor.cwd), &app.skills_dir, &config), config))
}

fn summary(skill: &Skill) -> Value {
  json!({"name": skill.name, "description": skill.description, "category": skill.category})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsQuery {
  /// Words describing a task: the skills that fit it, best first, instead of all of them.
  query: Option<String>,
}
pub async fn session_list(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<SkillsQuery>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  let (roots, config) = session_roots(&app, &id, &headers).await?;
  blocking(move || {
    let found = skills::discover(&roots);
    let usable = found.usable(&config);
    let shown = match query.query.as_deref().map(str::trim).filter(|query| !query.is_empty()) {
      Some(query) => skills::find(&usable, query),
      None => usable,
    };
    Ok(Json(json!({"skills": shown.into_iter().map(summary).collect::<Vec<_>>()})))
  })
  .await
}
pub async fn session_show(
  State(app): State<Arc<App>>,
  Path((id, name)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  let (roots, config) = session_roots(&app, &id, &headers).await?;
  blocking(move || {
    let found = skills::discover(&roots);
    let usable = found.usable(&config);
    // A name the model gets slightly wrong in case still finds the skill.
    let skill = usable
      .iter()
      .find(|skill| skill.name == name)
      .or_else(|| usable.iter().find(|skill| skill.name.eq_ignore_ascii_case(&name)))
      .ok_or_else(ApiError::not_found)?;
    Ok(Json(json!(skills::show(skill).map_err(ApiError::internal)?)))
  })
  .await
}

/// For settings: every skill in Wish's own and the configured directories - those another of the
/// same name keeps out and those switched off included - and what could not be read.
pub async fn list(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  let config = app.config_file.lock().await.config.skills.clone();
  let roots = skills::roots(None, &app.skills_dir, &config);
  let own = app.skills_dir.clone();
  blocking(move || {
    let found = skills::discover(&roots);
    let skills: Vec<Value> = found
      .skills
      .iter()
      .zip(found.standings())
      .map(|(skill, standing)| {
        json!({
          "name": skill.name, "description": skill.description, "category": skill.category,
          "path": skill.path, "dir": skill.dir, "source": skill.source,
          "disabled": config.disabled.contains(&skill.name), "standing": standing,
        })
      })
      .collect();
    let roots: Vec<Value> = roots
      .iter()
      .map(|root| json!({"source": root.source, "dir": root.dir, "exists": root.dir.is_dir()}))
      .collect();
    let problems: Vec<Value> = found
      .problems
      .iter()
      .map(|(path, message)| json!({"path": path, "message": message}))
      .collect();
    Ok(Json(json!({"dir": own, "roots": roots, "skills": skills, "problems": problems})))
  })
  .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShowQuery {
  /// Which of the skills of the name, by its directory; the first when not given.
  dir: Option<std::path::PathBuf>,
}
/// For settings: a skill's instructions and files, whether sessions see it or not.
pub async fn show(
  State(app): State<Arc<App>>,
  Path(name): Path<String>,
  Query(query): Query<ShowQuery>,
) -> Result<Json<Value>, ApiError> {
  let config = app.config_file.lock().await.config.skills.clone();
  let roots = skills::roots(None, &app.skills_dir, &config);
  blocking(move || {
    let found = skills::discover(&roots);
    let skill = found
      .skills
      .iter()
      .find(|skill| skill.name == name && query.dir.as_ref().is_none_or(|dir| skill.dir == *dir))
      .ok_or_else(ApiError::not_found)?;
    Ok(Json(json!(skills::show(skill).map_err(ApiError::internal)?)))
  })
  .await
}
