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
