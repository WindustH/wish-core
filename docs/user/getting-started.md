# Getting Started

Wish runs on your own machine as a single service with an embedded SQLite store. Packages and release archives bundle the compiled web app right beside the `wish` binary, so one process gives you both the agent runtime and its UI.

## Installing Wish

Pick whichever package manager you prefer:

```sh
npm install -g wish-agent               # Linux, macOS, and Windows
yay -S wish-agent-bin                   # Arch Linux (or wish-agent from source)
brew install windusth/tap/wish-agent    # macOS and Linux
```

On Arch Linux and Homebrew, `wish-agent` conflicts with Tk because both install a global `wish` command. If you'd rather avoid package managers, grab a standalone archive from [GitHub Releases](https://github.com/WindustH/wish-core/releases) and keep the `web/` folder next to the `wish` binary.

## First launch

Start Wish in a terminal:

```sh
wish
```

Without `--config`, Wish creates a starter `config.json` bound to `127.0.0.1:8790` on first launch using your OS's standard paths:

| Platform | Configuration file | Data directory |
| --- | --- | --- |
| Linux | `~/.config/wish-agent/config.json` (respects `$XDG_CONFIG_HOME`) | `~/.local/share/wish-agent` (respects `$XDG_DATA_HOME`) |
| macOS | `~/Library/Application Support/wish-agent/config.json` | `~/Library/Application Support/wish-agent/data` |
| Windows | `%APPDATA%\wish-agent\config.json` | `%LOCALAPPDATA%\wish-agent\data` |

You can always pass a custom config file with `wish --config /path/to/config.json`.

Open `http://127.0.0.1:8790` in a browser. A short onboarding flow helps you pick a model platform, sign in or enter an API key, select a default model, choose a working directory, and send your first message.

## How Wish works

### Sessions run on the host

Each conversation is an independent **session** with its own working directory, model choice, enabled tools, and file library. Because sessions run on the server rather than in your browser tab, you can kick off a long task on your laptop and check progress or reply from your phone later.

### Lean tools via `wish` commands

Instead of stuffing dozens of JSON tool schemas into every prompt—which inflates context and breaks prompt caching whenever you add an integration—Wish exposes most capabilities as subcommands of the `wish` CLI inside the session's shell: `wish skill`, `wish mcp`, `wish extension`, `wish task`, `wish blob`, `wish history`, `wish askuser`, and `wish session`. The model discovers and calls them only when a task calls for them.

If you turn off a session's shell (or switch a specific capability to direct tool mode), Wish advertises the enabled tools directly in the model request instead.

### Session files and local links

Every session has a file library (`$WISH_BLOB_ROOT` in its shell) alongside a scratch directory (`$WISH_TMPDIR`). You can upload files into a session or link existing files on your machine without duplicating bytes. When the model cites a file using a Markdown link or `wish://blob/...` reference, clicking it in the web app opens the image, jumps to the referenced PDF page, or seeks to the timestamp in audio or video.

## Built-in offline manual

Run `wish docs` for the chapter index, `wish docs configuration` for configuration details, or `wish docs architecture` for the engine’s design. `wish docs search <words>` searches the embedded text. These commands work outside a session and before the server is configured or started. See [Offline manual](../README.md#offline-manual) for all forms.

## Next steps

- Need to add more providers, proxies, MCP servers, or media generators? Head to [Configuration](configuration.md).
- Want to expose Wish safely over your LAN or a custom domain? See [Deployment](deployment.md).
