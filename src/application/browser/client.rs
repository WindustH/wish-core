//! One explicitly attached device per session. Commands never migrate to another attachment.
use super::{Error, Result, Room, SessionSlot};
use serde_json::{Value, json};
use std::{
  sync::{Arc, Mutex},
  time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const ACTIONS: &[&str] =
  &["open", "back", "observe", "read", "click", "fill", "select", "scroll"];

pub struct ClientCall {
  pub args: Value,
  pub result: oneshot::Sender<Result<Value>>,
}
struct Peer {
  id: Uuid,
  tx: mpsc::Sender<ClientCall>,
  closed: CancellationToken,
}
#[derive(Default)]
pub(super) struct Client(Mutex<Option<Peer>>);

impl Client {
  pub(super) fn disconnect(&self) {
    if let Some(peer) = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
      peer.closed.cancel()
    }
  }
  pub(super) fn connected(&self) -> bool {
    self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
  }
  pub(super) async fn call(&self, args: &Value) -> Result<Value> {
    if !ACTIONS.contains(&args["action"].as_str().unwrap_or_default()) {
      return Err(Error::invalid(
        "unsupported client browser action; list targets for available actions",
      ));
    }
    if args["tab"].is_string() {
      return Err(Error::invalid("client browser operates on the shared tab only"));
    }
    if args["action"] == "open" {
      super::actions::address(args)?;
    }
    let (result, rx) = oneshot::channel();
    {
      let peer = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      let peer = peer.as_ref().ok_or_else(|| Error::conflict("client browser is disconnected"))?;
      peer
        .tx
        .try_send(ClientCall { args: args.clone(), result })
        .map_err(|_| Error::conflict("client browser busy or disconnected"))?;
    }
    tokio::time::timeout(Duration::from_secs(20), rx)
      .await
      .map_err(|_| Error::unknown("client browser timed out; observe before retrying"))?
      .map_err(|_| Error::unknown("client browser disconnected; observe before retrying"))?
  }
}

pub struct ClientConnection {
  slot: Arc<SessionSlot>,
  id: Uuid,
  pub calls: mpsc::Receiver<ClientCall>,
  pub closed: CancellationToken,
}
impl ClientConnection {
  pub async fn attach(slot: Arc<SessionSlot>) -> Result<Self> {
    let _publication = slot.blob_publication.clone().read_owned().await;
    let Room { client, enabled, .. } = &slot.tools.browser;
    let mut guard = client.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    slot.require_live()?;
    if !enabled.load(std::sync::atomic::Ordering::Acquire) {
      return Err(Error::conflict("browser is disabled"));
    }
    if guard.is_some() {
      return Err(Error::conflict("a client browser is already connected"));
    }
    let (tx, calls) = mpsc::channel(1);
    let closed = CancellationToken::new();
    let id = Uuid::new_v4();
    *guard = Some(Peer { id, tx, closed: closed.clone() });
    drop(guard);
    Ok(Self { slot, id, calls, closed })
  }
}
impl Drop for ClientConnection {
  fn drop(&mut self) {
    let mut guard =
      self.slot.tools.browser.client.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.as_ref().is_some_and(|p| p.id == self.id) {
      guard.take();
    }
  }
}
pub(super) fn targets(room: &Room) -> Value {
  json!({"server":{"mode":"server","launch":"on_demand"},"client":{"mode":"client","connected":room.client.connected(),"actions":ACTIONS}})
}
