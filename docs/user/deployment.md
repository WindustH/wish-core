# Deployment

A Wish deployment needs four things: the `wish` binary, a `config.json`, a writable `data_dir`, and the built `web/` directory sitting beside the binary (or configured via `web_dir`).

## Building from source

Requires Rust and Node.js 22.19+:

```sh
git clone https://github.com/WindustH/wish-web.git
(cd wish-web && ./pnpmw install --frozen-lockfile && ./pnpmw build)

git clone https://github.com/WindustH/wish-core.git
cd wish-core
cargo build --release

mkdir -p ~/.local/lib/wish-agent ~/.local/bin
cp target/release/wish ~/.local/lib/wish-agent/wish
cp -r ../wish-web/dist ~/.local/lib/wish-agent/web
ln -sf ~/.local/lib/wish-agent/wish ~/.local/bin/wish
```

## Running as a systemd user service (Linux)

Create `~/.config/systemd/user/wish.service`:

```ini
[Unit]
Description=Wish AI Agent Service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=%h/.local/bin/wish --config %h/.config/wish-agent/config.json
Environment=WISH_HTTP_TOKEN=replace-with-a-strong-random-token
Restart=on-failure
RestartSec=3
TimeoutStopSec=40

[Install]
WantedBy=default.target
```

Enable it and keep it running outside interactive logins:

```sh
systemctl --user daemon-reload
systemctl --user enable --now wish.service
loginctl enable-linger "$USER"
```

Keeping `TimeoutStopSec` at 35 seconds or more gives running shell processes and SQLite WAL checkpoints enough time to finish on shutdown. On macOS, Homebrew provides `brew services start wish-agent`.

## Access control and remote devices

Always set `bearer_token_env` before exposing Wish beyond `127.0.0.1`. With a token set, all `/api/*` requests require `Authorization: Bearer <token>` while static assets, `/health`, and `/version` stay reachable so the web app can prompt for login. Without a token, Wish locks itself to loopback, `listen`, and `allowed_hosts` and blocks cross-origin mutations.

If Wish runs on a remote host and you sign in with ChatGPT or Google Antigravity OAuth, complete the browser login locally and paste the final `http://localhost:...` redirect URL into Wish's completion box.

### Public SSH access from a device-local Wish Web

The phone or computer runs its own Wish Web server on loopback. Its browser opens that local site. The Web server carries API requests, event streams and terminal WebSockets through a single SSH connection to Wish Core. Only the SSH port needs to be reachable remotely. Both sides opt in separately.

On the **backend**, add these fields to your configuration (paths are examples):

```json
{
  "listen": "127.0.0.1:9780",
  "ssh": {
    "listen": "0.0.0.0:2222",
    "authorized_keys": "/path/to/wish-users.pub",
    "host_key": "/path/to/wish-host-ed25519",
    "rekey_seconds": 3600
  }
}
```

Create the allowlist file before starting. It can be empty initially. Paste one user's OpenSSH public key per line. Blank lines and comments are accepted. Wish checks the file on each login. Removing a key prevents new logins; restarting the server also ends existing connections. Each authorized user can access this Wish instance with its existing application permissions and bearer-token policy.

Omit `host_key` to use a readable system SSH host private key (Ed25519, ECDSA, then RSA). Otherwise supply an explicit service key, for example one created with `ssh-keygen -t ed25519 -f /path/to/wish-host-ed25519` and an empty passphrase for unattended startup. The startup log prints the SHA256 host fingerprint for verifying first contact.

The SSH listener only accepts signed public-key authentication and local forwarding to this instance's loopback HTTP port. Wish clients prefer bidirectional `zlib@openssh.com` compression. It starts after public-key authentication and compresses each SSH payload before encryption. Clients that offer only `none` still connect. SSH renegotiates traffic keys after an hour by default or at the 1 GiB per-direction limit. See [device-side login and key management](https://github.com/WindustH/wish-web/blob/master/docs/user/deployment.md#device-local-ssh-client).

### Putting Wish behind HTTPS

For full mobile PWA support, notifications, and TLS, leave Wish bound to `127.0.0.1:9780` behind Caddy or Nginx. Disable response buffering so SSE streams arrive immediately, and raise the max upload size for 1 GiB Library uploads.

Caddy:
```caddyfile
wish.example.com {
    reverse_proxy 127.0.0.1:9780 {
        flush_interval -1
    }
}
```

Nginx:
```nginx
location / {
    proxy_pass http://127.0.0.1:9780;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_buffering off;
    proxy_read_timeout 3600s;
    client_max_body_size 1024m;
}
```

You can also host the frontend separately with [Wish Web](https://github.com/WindustH/wish-web/blob/master/docs/user/deployment.md).

## Upgrades, backups, and health checks

Stop Wish (or use `sqlite3 .backup`) before copying `data_dir` and `config.json`.

When upgrading, replace the `wish` binary and `web/` directory together and restart. If a newer version upgrades the database format, Wish automatically snapshots `wish.sqlite`, `management.sqlite`, and `config.json` into `<data_dir>/backups/` before modifying anything, so rolling back is simply restoring from that snapshot while stopped.

For monitoring, `GET /health` and `GET /version` are public; authenticated `GET /api/status` and `GET /api/storage` report live session counts, queue activity, and disk usage.
