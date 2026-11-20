//! Public-key-only SSH ingress. Each accepted channel can reach this Wish instance only.
use crate::application::config::SshSettings;
use russh::{
  Channel, ChannelOpenFailure, MethodKind,
  keys::{Algorithm, HashAlg, PublicKey, load_secret_key},
  server::{self, Auth, Msg},
};
use std::{
  io,
  net::SocketAddr,
  path::{Path, PathBuf},
  sync::Arc,
  time::Duration,
};
use tokio::{
  net::{TcpListener, TcpStream},
  sync::{Semaphore, oneshot},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub struct Endpoint {
  listener: TcpListener,
  config: Arc<server::Config>,
  keys: Arc<Path>,
  target: SocketAddr,
}

async fn authorized_keys(path: &Path) -> io::Result<Vec<PublicKey>> {
  let bytes = crate::utils::bounded_file::read(path, 1024 * 1024)
    .await
    .map_err(|e| io::Error::other(format!("read ssh.authorized_keys {}: {e}", path.display())))?;
  let text =
    std::str::from_utf8(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
  let mut keys = Vec::new();
  for (i, line) in text.lines().enumerate() {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
      continue;
    }
    keys.push(PublicKey::from_openssh(line).map_err(|_| {
      io::Error::other(format!(
        "expected a plain OpenSSH public key in {} line {}",
        path.display(),
        i + 1
      ))
    })?);
  }
  Ok(keys)
}

fn default_host_key() -> io::Result<PathBuf> {
  let dir = if cfg!(windows) {
    PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into()))
      .join("ssh")
  } else {
    PathBuf::from("/etc/ssh")
  };
  ["ssh_host_ed25519_key", "ssh_host_ecdsa_key", "ssh_host_rsa_key"]
    .iter()
    .map(|name| dir.join(name))
    .find(|path| std::fs::File::open(path).is_ok())
    .ok_or_else(|| io::Error::other("no readable system SSH host key; set ssh.host_key"))
}

impl Endpoint {
  pub async fn bind(settings: &SshSettings, target: SocketAddr) -> io::Result<Self> {
    let path = match &settings.host_key {
      Some(path) => path.clone(),
      None => default_host_key()?,
    };
    let key = load_secret_key(&path, None)
      .map_err(|e| io::Error::other(format!("load SSH host key {}: {e}", path.display())))?;
    let fingerprint = key.public_key().fingerprint(HashAlg::Sha256);
    authorized_keys(&settings.authorized_keys).await?;
    let keys = Arc::from(settings.authorized_keys.as_path());

    let config = server::Config {
      keys: vec![key],
      preferred: russh::Preferred {
        compression: std::borrow::Cow::Borrowed(&[
          russh::compression::ZLIB_LEGACY,
          russh::compression::NONE,
        ]),
        ..Default::default()
      },
      methods: (&[MethodKind::PublicKey][..]).into(),
      limits: russh::Limits {
        rekey_time_limit: Duration::from_secs(settings.rekey_seconds.into()),
        ..Default::default()
      },
      nodelay: true,
      ..Default::default()
    };
    let listener = TcpListener::bind(settings.listen).await?;
    eprintln!("wish SSH listening on {} (host key {fingerprint})", listener.local_addr()?);
    Ok(Self { listener, config: Arc::new(config), keys, target })
  }

  pub async fn serve(mut self, stop: CancellationToken, tasks: TaskTracker) -> io::Result<()> {
    let slots = Arc::new(Semaphore::new(128));
    let forwards = Arc::new(Semaphore::new(512));
    loop {
      let permit = tokio::select! {
        _ = stop.cancelled() => return Ok(()),
        permit = slots.clone().acquire_owned() => permit.expect("open semaphore"),
      };
      let (tcp, _) = tokio::select! {
        _ = stop.cancelled() => return Ok(()),
        result = axum::serve::Listener::accept(&mut self.listener) => result,
      };
      let (authenticated, proof) = oneshot::channel();
      let client = Client {
        keys: self.keys.clone(),
        target: self.target,
        authenticated: Some(authenticated),
        offered: None,
        channels: Arc::new(Semaphore::new(32)),
        forwards: forwards.clone(),
        tasks: tasks.clone(),
        stop: stop.clone(),
      };
      let config = self.config.clone();
      let stop = stop.clone();
      tasks.spawn(async move {
        let _permit = permit;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut session = tokio::select! {
          _ = stop.cancelled() => return,
          result = tokio::time::timeout_at(deadline, server::run_stream(config, tcp, client)) => {
            let Ok(Ok(session)) = result else { return };
            session
          }
        };
        let handle = session.handle();
        tokio::select! {
          _ = &mut session => return,
          _ = stop.cancelled() => {},
          proof = tokio::time::timeout_at(deadline, proof) => {
            if matches!(proof, Ok(Ok(()))) {
              tokio::select! { _ = &mut session => return, _ = stop.cancelled() => {} }
            }
          }
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
          let _ = handle
            .disconnect(russh::Disconnect::ByApplication, "Connection closed".into(), "".into())
            .await;
          let _ = session.await;
        })
        .await;
      });
    }
  }
}

struct Client {
  keys: Arc<Path>,
  target: SocketAddr,
  authenticated: Option<oneshot::Sender<()>>,
  offered: Option<Vec<PublicKey>>,
  channels: Arc<Semaphore>,
  forwards: Arc<Semaphore>,
  tasks: TaskTracker,
  stop: CancellationToken,
}
impl Client {
  fn check(allowed: &[PublicKey], key: &PublicKey, algorithm: &Algorithm) -> Auth {
    if !matches!(algorithm, Algorithm::Rsa { hash: None })
      && allowed.iter().any(|allowed| allowed.key_data() == key.key_data())
    {
      Auth::Accept
    } else {
      Auth::reject()
    }
  }
}
impl server::Handler for Client {
  type Error = russh::Error;
  async fn auth_publickey_offered(
    &mut self,
    _: &str,
    offer: server::PublicKeyOffer<'_>,
  ) -> Result<Auth, Self::Error> {
    if self.offered.is_none() {
      self.offered = Some(authorized_keys(&self.keys).await.unwrap_or_default());
    }
    Ok(Self::check(self.offered.as_deref().unwrap_or_default(), offer.public_key, offer.algorithm))
  }
  async fn auth_publickey(
    &mut self,
    _: &str,
    key: server::VerifiedPublicKey<'_>,
  ) -> Result<Auth, Self::Error> {
    // Recheck the authoritative file after proof so a revoked key cannot log in.
    Ok(Self::check(
      &authorized_keys(&self.keys).await.unwrap_or_default(),
      key.public_key,
      key.algorithm,
    ))
  }
  async fn auth_succeeded(&mut self, _: &mut server::Session) -> Result<(), Self::Error> {
    self.offered = None;
    if let Some(done) = self.authenticated.take() {
      let _ = done.send(());
    }
    Ok(())
  }
  async fn channel_open_direct_tcpip(
    &mut self,
    channel: Channel<Msg>,
    host: &str,
    port: u32,
    _: &str,
    _: u32,
    reply: server::ChannelOpenHandle,
    _: &mut server::Session,
  ) -> Result<(), Self::Error> {
    if (!matches!(host, "localhost" | "::1" | "127.0.0.1") && host != self.target.ip().to_string())
      || port != u32::from(self.target.port())
    {
      reply.reject(ChannelOpenFailure::AdministrativelyProhibited).await;
      return Ok(());
    }
    let (Ok(channel_slot), Ok(global_slot)) =
      (self.channels.clone().try_acquire_owned(), self.forwards.clone().try_acquire_owned())
    else {
      reply.reject(ChannelOpenFailure::ResourceShortage).await;
      return Ok(());
    };
    let Ok(Ok(mut tcp)) =
      tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(self.target)).await
    else {
      reply.reject(ChannelOpenFailure::ConnectFailed).await;
      return Ok(());
    };
    tcp.set_nodelay(true)?;
    reply.accept().await;
    let stop = self.stop.clone();
    self.tasks.spawn(async move {
      let _slots = (channel_slot, global_slot);
      let mut stream = channel.into_stream();
      tokio::select! {
        _ = stop.cancelled() => {}
        _ = tokio::io::copy_bidirectional(&mut tcp,&mut stream) => {}
      }
    });
    Ok(())
  }
}
