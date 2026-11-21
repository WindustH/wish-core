use super::*;
use base64::Engine;
use serde_json::json;

impl<T: Transport + Send + Sync> Client<T> {
  pub(super) async fn apply_files(
    &self,
    body: &mut Value,
    images: &Images<'_>,
    session: &str,
    config: &Resource<FileProtocol>,
  ) -> Result<(), Error> {
    for (token, encoded, mime) in images.bindings().filter(|(_, encoded, _)| !encoded.is_empty()) {
      let key = (session.to_owned(), Sha256::digest(encoded.as_bytes()).into());
      let cell = {
        let mut cache = self.resource_cache.files.lock().unwrap();
        if cache
          .peek(&key)
          .and_then(|entry| entry.get())
          .is_some_and(|(until, _)| *until <= Instant::now())
        {
          cache.pop(&key);
        }
        cache.get_or_insert(key, FileCell::default).clone()
      };
      let (_, reference) = cell
        .get_or_try_init(|| async {
          let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| Error::Build("invalid model image encoding".into()))?;
          let (ttl, id) = match self.upload(config, bytes, mime).await {
            Ok(id) => (
              if matches!(config.protocol, FileProtocol::Deepseek | FileProtocol::Openai) {
                86000
              } else {
                3500
              },
              Some(id),
            ),
            Err(Error::Upstream { status: Some(400 | 403 | 404 | 413 | 415 | 422), .. }) => {
              (60, None)
            }
            Err(error) => return Err(error),
          };
          Ok::<_, Error>((Instant::now() + Duration::from_secs(ttl), id))
        })
        .await?;
      if let Some(reference) = reference {
        replace(body, token, mime, reference, config.protocol);
      }
    }
    Ok(())
  }
  async fn upload(
    &self,
    config: &Resource<FileProtocol>,
    bytes: Vec<u8>,
    mime: &str,
  ) -> Result<String, Error> {
    let extension = match mime {
      "image/png" => "png",
      "image/jpeg" => "jpg",
      "image/webp" => "webp",
      "image/gif" => "gif",
      _ => return Err(Error::Build("unsupported image upload type".into())),
    };
    if config.protocol == FileProtocol::Gemini {
      let mut start =
        Draft::post_json(&json!({"file":{"display_name":format!("image.{extension}")}}))?;
      start.path = Some(config.path.clone());
      start.headers.extend([
        ("X-Goog-Upload-Protocol".into(), "resumable".into()),
        ("X-Goog-Upload-Command".into(), "start".into()),
        ("X-Goog-Upload-Header-Content-Length".into(), bytes.len().to_string()),
        ("X-Goog-Upload-Header-Content-Type".into(), mime.into()),
      ]);
      let mut call =
        self.endpoint.build_call(start, &self.credentials, crate::utils::time::unix_seconds())?;
      let reply = self
        .transport
        .execute(&call)
        .await?
        .require_success(|s, b| self.decode_upstream_error(s, b))?;
      let url = reply
        .get_header("x-goog-upload-url")
        .and_then(|s| reqwest::Url::parse(s).ok())
        .filter(|u| {
          reqwest::Url::parse(&call.url).is_ok_and(|origin| origin.origin() == u.origin())
            && u.username().is_empty()
            && u.password().is_none()
        })
        .ok_or_else(|| Error::Malformed("invalid Gemini file upload destination".into()))?;
      call.url = url.to_string();
      call.headers = vec![
        ("Content-Type".into(), mime.into()),
        ("X-Goog-Upload-Offset".into(), "0".into()),
        ("X-Goog-Upload-Command".into(), "upload, finalize".into()),
      ];
      call.body = bytes.into();
      super::log_transfer("file", call.body.len(), 0);
      let (_, reply) = self.read_reply(self.transport.execute(&call).await?)?;
      return reply
        .pointer("/file/uri")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Error::Malformed("missing Gemini file URI".into()));
    }
    let fields = match config.protocol {
      FileProtocol::Deepseek | FileProtocol::Openai => vec![
        ("purpose", "user_data".into()),
        ("expires_after[anchor]", "created_at".into()),
        ("expires_after[seconds]", "86400".into()),
      ],
      FileProtocol::Kimi => vec![("purpose", "image".into())],
      _ => vec![],
    };
    let mut draft = crate::protocol::endpoint::multipart_file(&fields, &bytes, mime, extension);
    draft.path = Some(config.path.clone());
    if config.protocol == FileProtocol::Anthropic {
      draft.headers.extend([
        ("anthropic-version".into(), "2023-06-01".into()),
        ("anthropic-beta".into(), "files-api-2025-04-14".into()),
      ]);
    }
    self.resource_json(draft).await?["id"]
      .as_str()
      .filter(|s| !s.is_empty())
      .map(str::to_owned)
      .ok_or_else(|| Error::Malformed("missing uploaded file id".into()))
  }
}
fn replace(value: &mut Value, token: &str, mime: &str, id: &str, protocol: FileProtocol) {
  let data_url = |v: Option<&str>| {
    v.and_then(|s| s.strip_prefix("data:"))
      .and_then(|s| s.split_once(";base64,"))
      .is_some_and(|(m, t)| m == mime && t == token)
  };
  if value["type"] == "input_image" && data_url(value["image_url"].as_str()) {
    let object = value.as_object_mut().unwrap();
    object.remove("image_url");
    object.insert("file_id".into(), json!(id));
    return;
  }
  if value["type"] == "image_url"
    && data_url(value.pointer("/image_url/url").and_then(Value::as_str))
  {
    if protocol == FileProtocol::Kimi {
      value["image_url"]["url"] = json!(format!("ms://{id}"));
    } else {
      *value = json!({"type":"file","file_id":id});
    }
    return;
  }
  if value["type"] == "image"
    && value.pointer("/source/data").and_then(Value::as_str) == Some(token)
  {
    value["source"] = json!({"type":"file","file_id":id});
    return;
  }
  if value.pointer("/inlineData/data").and_then(Value::as_str) == Some(token) {
    *value = json!({"fileData":{"fileUri":id,"mimeType":mime}});
    return;
  }
  if value["type"] == "image" && value["data"].as_str() == Some(token) {
    value.as_object_mut().unwrap().remove("data");
    value["uri"] = json!(id);
    return;
  }
  match value {
    Value::Array(a) => a.iter_mut().for_each(|v| replace(v, token, mime, id, protocol)),
    Value::Object(o) => o.values_mut().for_each(|v| replace(v, token, mime, id, protocol)),
    _ => (),
  }
}

pub(super) fn object_location(value: &mut Value, token: &str, mime: &str, uri: &str) {
  if value.pointer("/image/source/bytes").and_then(Value::as_str) == Some(token) {
    value["image"]["source"] = json!({"s3Location":{"uri":uri}});
    return;
  }
  if value.pointer("/inlineData/data").and_then(Value::as_str) == Some(token) {
    *value = json!({"fileData":{"fileUri":uri,"mimeType":mime}});
    return;
  }
  if value
    .pointer("/image_url/url")
    .and_then(Value::as_str)
    .and_then(|s| s.split_once(";base64,"))
    .is_some_and(|(_, t)| t == token)
  {
    value["image_url"]["url"] = json!(uri);
    return;
  }
  match value {
    Value::Array(a) => a.iter_mut().for_each(|v| object_location(v, token, mime, uri)),
    Value::Object(o) => o.values_mut().for_each(|v| object_location(v, token, mime, uri)),
    _ => (),
  }
}
