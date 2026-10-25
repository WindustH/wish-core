use super::*;
use crate::protocol::attempt::{Call, Reply, Transport};
use crate::protocol::endpoint::Credentials;
use serde_json::json;
use std::sync::Mutex;
struct Capture {
  calls: Mutex<Vec<Call>>,
  status: u16,
  body: Value,
}
impl Transport for Capture {
  type Stream = <crate::transport::ReqwestTransport as Transport>::Stream;
  async fn execute(&self, call: &Call) -> Result<Reply, Error> {
    self.calls.lock().unwrap().push(call.clone());
    Ok(Reply {
      status: self.status,
      headers: vec![],
      body: serde_json::to_vec(&self.body).unwrap(),
    })
  }
  async fn execute_stream(&self, _: &Call) -> Result<Self::Stream, Error> {
    unreachable!()
  }
}
#[tokio::test]
async fn readers_preserve_wire_shapes_and_normalize_replies() {
  for (protocol, reply, path) in [
    (FetchProtocol::DeepseekFetch, json!({"title":"Page","content":"markdown"}), "/v1/tools/fetch"),
    (
      FetchProtocol::ZaiReader,
      json!({"reader_result":{"title":"Page","content":"markdown"}}),
      "/reader",
    ),
    (FetchProtocol::JinaReader, json!({"data":{"title":"Page","content":"markdown"}}), "/"),
    (FetchProtocol::CodexFetch, json!({"title":"Page","output":"markdown"}), "/alpha/search"),
  ] {
    let transport = Capture { calls: Mutex::new(vec![]), status: 200, body: reply };
    let credentials = Credentials {
      api_key: "test-key".into(),
      account_id: Some("test-account".into()),
      ..Default::default()
    };
    let request = FetchRequest {
      url: "https://example.org/page".into(),
      refresh: true,
      conversation: Some("conversation".into()),
      ..Default::default()
    };
    let page = fetch(
      &transport,
      protocol,
      &credentials,
      Some("https://reader.test"),
      &[("user-agent".into(), "wish-test".into())],
      &request,
      0,
    )
    .await
    .unwrap();
    assert_eq!(page, FetchResult { title: "Page".into(), content: "markdown".into() });
    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.url, format!("https://reader.test{path}"));
    assert!(
      call
        .headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "Bearer test-key")
    );
    let body: Value = serde_json::from_slice(&call.body).unwrap();
    match protocol {
      FetchProtocol::CodexFetch => {
        assert_eq!(body["commands"]["open"][0]["ref_id"], request.url);
        assert_eq!(body["id"], "conversation");
        assert_eq!(body["model"], "gpt-5.5");
        assert!(call.headers.iter().any(|(k, v)| k == "chatgpt-account-id" && v == "test-account"));
        assert!(call.headers.iter().any(|(k, v)| k == "originator" && v == "codex_cli_rs"));
      }
      FetchProtocol::ZaiReader => {
        assert_eq!(
          body,
          json!({"url":request.url,"return_format":"markdown","no_cache":true,"keep_img_data_url":false})
        );
      }
      FetchProtocol::JinaReader => {
        assert_eq!(body, json!({"url":request.url}));
        assert!(call.headers.iter().any(|(k, v)| k == "x-no-cache" && v == "true"));
      }
      FetchProtocol::DeepseekFetch => assert_eq!(body, json!({"url":request.url})),
    }
  }
}
#[tokio::test]
async fn optional_auth_and_failures_remain_protocol_concerns() {
  let transport =
    Capture { calls: Mutex::new(vec![]), status: 200, body: json!({"data":{"content":"text"}}) };
  let request = FetchRequest { url: "https://example.org".into(), ..Default::default() };
  fetch(&transport, FetchProtocol::JinaReader, &Credentials::default(), None, &[], &request, 0)
    .await
    .unwrap();
  assert!(
    !transport.calls.lock().unwrap()[0]
      .headers
      .iter()
      .any(|(k, _)| k.eq_ignore_ascii_case("authorization") || k == "x-no-cache")
  );
  let transport = Capture {
    calls: Mutex::new(vec![]),
    status: 429,
    body: json!({"error":{"message":"rate limited"}}),
  };
  let error =
    fetch(&transport, FetchProtocol::JinaReader, &Credentials::default(), None, &[], &request, 0)
      .await
      .unwrap_err();
  assert!(matches!(error, Error::Upstream { status: Some(429), .. }));
  assert_eq!(transport.calls.lock().unwrap().len(), 1, "protocol does not retry");
  for protocol in FetchProtocol::ALL {
    assert!(matches!(protocol.parse_body(&json!({})), Err(Error::Malformed(_))));
  }
}
