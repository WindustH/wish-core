use super::*;
use serde_json::json;

impl<T: Transport + Send + Sync> Client<T> {
  pub(super) async fn apply_prefix(
    &self,
    body: &mut Value,
    images: &Images<'_>,
    session: &str,
    model: &str,
    config: &Resource<PrefixProtocol>,
  ) -> Result<(), Error> {
    let google = config.protocol != PrefixProtocol::Ark;
    let items = if google { "contents" } else { "input" };
    let turns = body[items]
      .as_array()
      .ok_or_else(|| Error::Build("prefix wire has no generation inputs".into()))?;
    let key = (session.to_owned(), model.to_owned());
    let model_resource = match config.protocol {
      PrefixProtocol::Vertex if !model.starts_with("projects/") => {
        let mut draft = Draft::get(vec![]);
        draft.path = Some(config.path.clone());
        let call =
          self.endpoint.build_call(draft, &self.credentials, crate::utils::time::unix_seconds())?;
        let url = reqwest::Url::parse(&call.url)
          .map_err(|_| Error::Build("invalid Vertex cache collection".into()))?;
        let Some((_, resource)) = url.path().split_once("/projects/") else {
          return Err(Error::Build("Vertex prefix cache requires a project and location".into()));
        };
        let Some(parent) = resource.strip_suffix("/cachedContents") else {
          return Err(Error::Build("expected Vertex cachedContents collection".into()));
        };
        format!("projects/{parent}/publishers/google/models/{model}")
      }
      PrefixProtocol::Gemini if !model.starts_with("models/") => format!("models/{model}"),
      _ => model.to_owned(),
    };
    let mut seed = json!({"model":model_resource});
    let inherited = &["systemInstruction", "tools", "toolConfig"];
    if google {
      for &field in inherited {
        if let Some(value) = body.get(field) {
          seed[field] = value.clone();
        }
      }
    } else {
      seed["caching"] = json!({"type":"enabled","prefix":true});
      seed["store"] = json!(true);
      seed["stream"] = json!(false);
      for field in ["tools", "instructions", "reasoning"] {
        if let Some(value) = body.get(field) {
          seed[field] = value.clone();
        }
      }
    }
    let settings = Sha256::digest(images.serialize(&seed)?).into();
    // Start the live suffix at a user message, leaving a complete exchange in the
    // prefix. Keep using that immutable prefix as the conversation grows.
    let cached = self
      .resource_cache
      .prefixes
      .lock()
      .unwrap()
      .get(&key)
      .filter(|p| p.until > Instant::now() && p.settings == settings)
      .cloned();
    let assistant = if google { "model" } else { "assistant" };
    let Some(completed) =
      turns.iter().position(|t| t["role"] == assistant || t["type"] == "function_call")
    else {
      return Ok(());
    };
    let latest = turns.iter().enumerate().skip(completed + 1).rev().find_map(|(i, t)| {
      (t["role"] == "user" && t.pointer("/parts/0/functionResponse").is_none()).then_some(i)
    });
    let Some(latest) = latest else {
      return Ok(());
    };
    let cut = cached
      .as_ref()
      .map(|p| p.digests.len())
      .filter(|&n| {
        n <= latest
          && turns[n]["role"] == "user"
          && turns[n].pointer("/parts/0/functionResponse").is_none()
      })
      .unwrap_or(latest);
    let mut size = 0usize;
    let digests = turns[..cut]
      .iter()
      .map(|item| {
        let bytes = images.serialize(item)?;
        size += bytes.len();
        Ok::<[u8; 32], Error>(Sha256::digest(bytes).into())
      })
      .collect::<Result<Vec<_>, _>>()?;
    let cached = cached.filter(|p| p.digests == digests);
    let id = if let Some(entry) = cached {
      let Some(id) = entry.id else { return Ok(()) };
      id
    } else {
      if size + seed.to_string().len() < 4096 {
        return Ok(());
      }
      seed[items] = Value::Array(turns[..cut].to_vec());
      if google {
        seed["ttl"] = json!("3600s");
      }
      let draft = Draft::post(
        Some(config.path.clone()),
        vec![("content-type".into(), "application/json".into())],
        images.serialize(&seed)?,
      );
      let response = match self.resource_json(draft).await {
        Ok(response) => response,
        Err(Error::Upstream { status: Some(400 | 403 | 404 | 422), .. }) => {
          self.resource_cache.prefixes.lock().unwrap().put(
            key,
            Prefix { settings, digests, id: None, until: Instant::now() + Duration::from_secs(60) },
          );
          return Ok(());
        }
        Err(error) => return Err(error),
      };
      let field = if google { "name" } else { "id" };
      let id = response[field]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Malformed("missing prefix cache id".into()))?
        .to_owned();
      self.resource_cache.prefixes.lock().unwrap().put(
        key,
        Prefix {
          settings,
          digests,
          id: Some(id.clone()),
          until: Instant::now() + Duration::from_secs(3500),
        },
      );
      id
    };
    body[items].as_array_mut().unwrap().drain(..cut);
    if google {
      for &field in inherited {
        body.as_object_mut().unwrap().remove(field);
      }
      body["cachedContent"] = json!(id);
    } else {
      body["store"] = json!(true);
      body["caching"] = json!({"type":"enabled"});
      body["previous_response_id"] = json!(id);
    }
    Ok(())
  }
}
