# Deployment

This guide runs Wish as a long-lived service on Linux with systemd. The same
ideas apply elsewhere: one `wish` process, one configuration file, one data
directory.

- [Install](#install)
- [Run as a service](#run-as-a-service)
- [Access control](#access-control)
- [Reaching Wish from other devices](#reaching-wish-from-other-devices)
- [Restarts and shutdown](#restarts-and-shutdown)
- [Backups and upgrades](#backups-and-upgrades)
- [Monitoring](#monitoring)

## Install

```sh
git clone https://github.com/WindustH/wish-core.git
cd wish-core
cargo build --release
install -Dm755 target/release/wish ~/.local/bin/wish
```

The binary is self-contained: SQLite is built in and TLS uses bundled root
certificates, so it can be copied to another machine with the same OS and
architecture.

A layout that works well:

| Path | Purpose |
| --- | --- |
| `~/.config/wish/config.json` | [Configuration](configuration.md), writable by Wish |
| `~/.config/wish/wish.env` | API keys and the access token, mode `600` |
| `~/.local/share/wish/` | `data_dir` |

Set `data_dir` to an absolute path so it does not depend on where Wish is
started.

## Run as a service

Wish's shell runs commands as the user Wish runs as, with that user's files and
tools, so a user service is usually what you want.
`~/.config/systemd/user/wish.service`:

```ini
[Unit]
Description=Wish agent server
After=network-online.target

[Service]
ExecStart=%h/.local/bin/wish --config %h/.config/wish/config.json
EnvironmentFile=%h/.config/wish/wish.env
WorkingDirectory=%h
Restart=on-failure
TimeoutStopSec=60

[Install]
WantedBy=default.target
```

`~/.config/wish/wish.env`:

```sh
WISH_HTTP_TOKEN=a-long-random-string
OPENAI_API_KEY=sk-...
```

```sh
systemctl --user daemon-reload
systemctl --user enable --now wish
loginctl enable-linger "$USER"   # keep it running after you log out
journalctl --user -u wish -f     # logs
```

Commands inherit the service's environment, including `PATH`. If the agent
needs tools from your interactive shell setup, configure a login shell such as
zsh or bash (its default arguments `-lc` load your profile). See
[shell](configuration.md#shell).

## Access control

By default Wish listens on `127.0.0.1` without authentication, which is fine
for a single-user machine. Anyone who can reach the API can run commands as
you, so before anything else can connect:

1. Set `"bearer_token_env": "WISH_HTTP_TOKEN"` and define that variable (as
   above). Every `/api` request then needs
   `Authorization: Bearer <token>`.
2. Give the same token to the web app's server (`WISH_HTTP_TOKEN` for
   `serve.ts`). It adds the header on the browser's behalf, so the token
   never reaches the browser.

Wish has no user accounts or per-user permissions. It is designed for one
trusted person.

## Reaching Wish from other devices

Keep Wish itself on loopback and publish the web app instead. Its server
forwards `/api` to Wish, checks the `Host` header and blocks cross-site
requests. For access beyond your local network, put it behind a reverse proxy
with HTTPS. Installing the web app on a phone also needs HTTPS. The
[web app deployment guide](https://github.com/WindustH/wish-web/blob/master/docs/en/deployment.md)
covers both.

The web app can also connect to a Wish server directly: sign out, then enter
the server's address and access token on the sign-in page. This is how one
installed copy of the app switches between several servers. Wish accepts such
cross-origin connections only when it requires a token. Set `listen` to a
reachable address and serve it over HTTPS through a reverse proxy, since a page
opened over HTTPS cannot connect to a plain HTTP address. Streaming responses
use Server-Sent Events, so disable response buffering for `/api` in the proxy.

## Restarts and shutdown

On `SIGTERM` or `SIGINT` (`systemctl stop`, Ctrl-C) Wish:

1. stops accepting new work,
2. interrupts running tasks and waits for them to save what the model had
   produced and for running commands to be cleaned up,
3. stops background commands,
4. flushes the database and exits.

This usually takes a moment; `TimeoutStopSec` above leaves room for it.
Interrupted tasks are not resumed automatically after a restart; send the
session a message to continue. If the process is killed
abruptly, sessions caught mid-command are marked as unfinished when next
opened, and Wish never repeats a command whose effects are unknown.

To avoid interrupting anything, check that nothing is running first:

```sh
curl -s -H "Authorization: Bearer $WISH_HTTP_TOKEN" http://127.0.0.1:9780/api/status
# "queue": {"active_sessions": 0, "compacting_sessions": 0, ...}
```

## Backups and upgrades

**Back up** the data directory and the configuration file. With Wish stopped,
copying the directory is enough. While it runs, copy the databases through
SQLite and the rest as files:

```sh
cd ~/.local/share/wish
sqlite3 wish.sqlite ".backup /backup/wish.sqlite"
sqlite3 management.sqlite ".backup /backup/management.sqlite"
rsync -a blobs shell /backup/
```

**Upgrade** by building the new version and swapping the binary. Keep the old
one for a quick rollback:

```sh
cargo build --release
cp ~/.local/bin/wish ~/.local/bin/wish.previous
cp target/release/wish ~/.local/bin/wish.new
mv ~/.local/bin/wish.new ~/.local/bin/wish   # replacing in place fails while it runs
systemctl --user restart wish
```

Back up before upgrading. A database that a newer release has written to may
not open with an older one, so rolling back the binary may also mean restoring
the backup.

## Monitoring

| Check | Meaning |
| --- | --- |
| `GET /health` | The process is up (no authentication) |
| `GET /version` | Running version |
| `GET /api/status` | Stored sessions, running and queued work, uptime |
| `GET /api/storage` | Disk usage by category |

Errors that do not belong to a request, such as failed credential refreshes,
are written to standard error and end up in the service log.
