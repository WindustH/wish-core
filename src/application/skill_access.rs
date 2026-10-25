//! Skill access: what `wish skill` asks through the bridge, and the settings page's view of
//! them. A session's skills switch is enforced here and nowhere else.
use super::bridge_access as bridge;
use crate::application::error::{ApplicationError, blocking};
use crate::application::{
  app::App,
  config::SkillsConfig,
  skills::{self, Root, Skill},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// Where the session a bridge request speaks for finds skills, when its skills switch is on.
async fn session_roots(
  app: &Arc<App>,
  id: &str,
  token: Option<&str>,
) -> Result<(Vec<Root>, SkillsConfig), ApplicationError> {
  let slot = bridge::authenticate_session(app, id, token).await?;
  // Said to the model through the command's output, so it knows why and who can change it.
  let descriptor = slot.get_descriptor();
  if !descriptor.tools.skills {
    return Err(ApplicationError::conflict(
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
  app: Arc<App>,
  id: String,
  query: SkillsQuery,
  token: Option<&str>,
) -> Result<Value, ApplicationError> {
  let (roots, config) = session_roots(&app, &id, token).await?;
  blocking(move || {
    let found = skills::discover(&roots);
    let usable = found.usable(&config);
    let shown = match query.query.as_deref().map(str::trim).filter(|query| !query.is_empty()) {
      Some(query) => skills::find(&usable, query),
      None => usable,
    };
    Ok(json!({"skills": shown.into_iter().map(summary).collect::<Vec<_>>()}))
  })
  .await
}
pub async fn session_show(
  app: Arc<App>,
  (id, name): (String, String),
  token: Option<&str>,
) -> Result<Value, ApplicationError> {
  let (roots, config) = session_roots(&app, &id, token).await?;
  blocking(move || {
    let found = skills::discover(&roots);
    let usable = found.usable(&config);
    // A name the model gets slightly wrong in case still finds the skill.
    let skill = usable
      .iter()
      .find(|skill| skill.name == name)
      .or_else(|| usable.iter().find(|skill| skill.name.eq_ignore_ascii_case(&name)))
      .ok_or_else(ApplicationError::not_found)?;
    Ok(json!(skills::show(skill).map_err(ApplicationError::internal)?))
  })
  .await
}

/// For settings: every skill in Wish's own and the configured directories - those another of the
/// same name keeps out and those switched off included - and what could not be read.
pub async fn list(app: Arc<App>) -> Result<Value, ApplicationError> {
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
    Ok(json!({"dir": own, "roots": roots, "skills": skills, "problems": problems}))
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
  app: Arc<App>,
  name: String,
  query: ShowQuery,
) -> Result<Value, ApplicationError> {
  let config = app.config_file.lock().await.config.skills.clone();
  let roots = skills::roots(None, &app.skills_dir, &config);
  blocking(move || {
    let found = skills::discover(&roots);
    let skill = found
      .skills
      .iter()
      .find(|skill| skill.name == name && query.dir.as_ref().is_none_or(|dir| skill.dir == *dir))
      .ok_or_else(ApplicationError::not_found)?;
    Ok(json!(skills::show(skill).map_err(ApplicationError::internal)?))
  })
  .await
}
