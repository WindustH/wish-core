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

Wish's packages install the program as `wish-agent`, with the web app beside
it:

```sh
npm install -g wish-agent               # Linux, macOS and Windows
yay -S wish-agent-bin                   # Arch Linux; wish-agent or wish-agent-git build from source
brew install windusth/tap/wish-agent    # macOS and Linux
```

Each [release](https://github.com/WindustH/wish-core/releases) also has an
archive per platform holding the program, `wish`, and the web app in `web/`.
Keep the two together: Wish looks for `web` beside its program.

To build from source you need a Rust toolchain and Node.js 22.19 or newer:

```sh
git clone https://github.com/WindustH/wish-web.git
(cd wish-web && ./pnpmw install --frozen-lockfile && ./pnpmw build)
git clone https://github.com/WindustH/wish-core.git
cd wish-core
cargo build --release
mkdir -p ~/.local/lib/wish-agent ~/.local/bin
cp target/release/wish ~/.local/lib/wish-agent/wish
cp -r ../wish-web/dist ~/.local/lib/wish-agent/web
ln -sf ~/.local/lib/wish-agent/wish ~/.local/bin/wish-agent
```

The program is self-contained: SQLite is built in and TLS uses bundled root
certificates, so it can be copied to another machine with the same OS and
architecture.

Started without `--config`, Wish uses its user's
[configuration file](configuration.md), writing one on the first start. On
Linux:

| Path | Purpose |
| --- | --- |
| `~/.config/wish-agent/config.json` | [Configuration](configuration.md), writable by Wish |
| `~/.config/wish-agent/wish.env` | API keys and the access token, mode `600`, for the service below |
| `~/.local/share/wish-agent/` | `data_dir` |

## Run as a service

Wish's shell runs commands as the user Wish runs as, with that user's files and
tools, so a user service is usually what you want. The Arch Linux packages
include one, and Homebrew has its own:

```sh
systemctl --user enable --now wish-agent   # Arch Linux
brew services start wish-agent             # Homebrew
```

Elsewhere, write `~/.config/systemd/user/wish-agent.service`:

```ini
[Unit]
Description=Wish agent server
After=network-online.target

[Service]
ExecStart=%h/.local/bin/wish-agent
EnvironmentFile=-%h/.config/wish-agent/wish.env
WorkingDirectory=%h
Restart=on-failure
TimeoutStopSec=60

[Install]
WantedBy=default.target
```

`~/.config/wish-agent/wish.env`:

```sh
WISH_HTTP_TOKEN=a-long-random-string
OPENAI_API_KEY=sk-...
```

```sh
systemctl --user daemon-reload
systemctl --user enable --now wish-agent
loginctl enable-linger "$USER"         # keep it running after you log out
journalctl --user -u wish-agent -f     # logs
```

Commands inherit the service's environment, including `PATH`. If the agent
needs tools from your interactive shell setup, configure a login shell such as
zsh or bash (its default arguments `-lc` load your profile). See
[shell](configuration.md#shell).

## Access control

By default Wish listens on `127.0.0.1` without authentication, which is fine
for a single-user machine. Without a token it answers only requests addressed
to it by its own name and refuses changes from other origins, so a web page
cannot drive it through your browser. Anyone who can reach the API can still
run commands as you, so before anything else can connect:

1. Set `"bearer_token_env": "WISH_HTTP_TOKEN"` and define that variable (as
   above). Every `/api` request then needs
   `Authorization: Bearer <token>`.
2. Open the web app: it asks for the token once, on its sign-in page, and
   keeps it in that browser. If you serve the web app apart from Wish with its
   own server (`serve.ts`), give the token to that server instead
   (`WISH_HTTP_TOKEN`); it adds the header itself, so the token never reaches
   the browser.

Wish has no user accounts or per-user permissions. It is designed for one
trusted person.

## Reaching Wish from other devices

Wish serves its web app itself, so reaching the app from another device means
reaching Wish. Set an access token first. On your local network, set `listen`
to the machine's address (or `0.0.0.0:8790`) and open
`http://<address>:8790`. For access beyond your network, and to install the
app on a phone, which needs HTTPS, put Wish behind a reverse proxy with HTTPS
and disable response buffering, since streaming responses use Server-Sent
Events. A trusted network can go without a token by listing the names it uses
in `allowed_hosts`. The web app can also be served apart from Wish; the
[web app deployment guide](https://github.com/WindustH/wish-web/blob/master/docs/en/deployment.md)
covers that.

The web app can also connect to another Wish server: sign out, then enter the
server's address and access token on the sign-in page. This is how one
installed copy of the app switches between several servers. Wish accepts such
cross-origin connections only when it requires a token, and a page opened over
HTTPS cannot connect to a plain HTTP address.

## Restarts and shutdown

On `SIGTERM` or `SIGINT` (`systemctl stop`, Ctrl-C) Wish:

1. stops accepting new work,
2. interrupts running tasks and waits for them to save what the model had
   produced and for running commands to be cleaned up,
3. stops background commands,
4. flushes the database and exits.

This usually takes a moment; `TimeoutStopSec` above leaves room for it.

On Windows the same shutdown follows Ctrl-C, Ctrl-Break, closing Wish's console
window or shutting Windows down. Windows ends the process about five seconds
after the console closes, so a slow shutdown can be cut short there; a service
wrapper should stop Wish with Ctrl-C.
Interrupted tasks are not resumed automatically after a restart; send the
session a message to continue. If the process is killed
abruptly, sessions caught mid-command are marked as unfinished when next
opened, and Wish never repeats a command whose effects are unknown.

To avoid interrupting anything, check that nothing is running first:

```sh
curl -s -H "Authorization: Bearer $WISH_HTTP_TOKEN" http://127.0.0.1:8790/api/status
# "queue": {"active_sessions": 0, "compacting_sessions": 0, ...}
```

## Backups and upgrades

**Back up** the data directory and the configuration file. With Wish stopped,
copying the directory is enough. While it runs, copy the databases through
SQLite and the rest as files:

```sh
cd ~/.local/share/wish-agent
sqlite3 wish.sqlite ".backup /backup/wish.sqlite"
sqlite3 management.sqlite ".backup /backup/management.sqlite"
rsync -a blobs shell /backup/
```

**Upgrade** through the package manager you installed with (`npm update -g
wish-agent`, `yay -Syu`, `brew upgrade wish-agent`), then restart the
service. A build from source swaps the program and the web app; keep the old
ones for a quick rollback:

```sh
cargo build --release
(cd ../wish-web && git pull && ./pnpmw install --frozen-lockfile && ./pnpmw build)
cp -r ~/.local/lib/wish-agent ~/.local/lib/wish-agent.previous
cp target/release/wish ~/.local/lib/wish-agent/wish.new
mv ~/.local/lib/wish-agent/wish.new ~/.local/lib/wish-agent/wish   # replacing in place fails while it runs
rm -rf ~/.local/lib/wish-agent/web && cp -r ../wish-web/dist ~/.local/lib/wish-agent/web
systemctl --user restart wish-agent
```

When a release changes how data or the configuration is stored, it migrates
them the first time it starts: the service log lists each step (`migrating
... from format 1 to 2`), and a copy of both databases and the configuration
file taken just before is kept in `backups/before-migration-<from>-to-<to>-<time>/`
in the data directory. If a step fails, nothing is changed and Wish refuses to
start, naming the step and the copy. Remove old copies once you are happy with
an upgrade.

A data directory a newer release has migrated does not open with an older one
(`written by a newer build`). To roll back past such an upgrade, restore the
databases and the configuration file from the copy the newer release took, then
start the older binary.

## Monitoring

| Check | Meaning |
| --- | --- |
| `GET /health` | The process is up (no authentication) |
| `GET /version` | Running version |
| `GET /api/status` | Stored sessions, running and queued work, uptime |
| `GET /api/storage` | Disk usage by category |

Errors that do not belong to a request, such as failed credential refreshes,
are written to standard error and end up in the service log.
