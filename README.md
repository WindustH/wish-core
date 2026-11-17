<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/wish-logo-dark.svg">
    <img src="docs/assets/wish-logo-light.svg" alt="Wish" width="300">
  </picture>
</p>

<p align="center">
  <a href="docs/user/README.md">User Docs</a> ·
  <a href="docs/developer/README.md">Developer Docs</a> ·
  <a href="https://github.com/WindustH/wish-web">Wish Web</a> ·
  <a href="README.zh-CN.md">简体中文</a>
</p>

---

Wish is a clean, capable, and reliable AI agent harness. It runs as a single Rust binary with embedded storage, paired with [Wish Web](https://github.com/WindustH/wish-web)—an interface crafted for both phones and desktops—so you can pick up your work comfortably from any device. Here's to turning your wishes into reality.

## Download & run

Install `wish-agent` (the web interface is already included):

```sh
npm install -g wish-agent               # Linux, macOS, and Windows
yay -S wish-agent-bin                   # Arch Linux
brew install windusth/tap/wish-agent    # macOS and Linux
```

You can also grab prebuilt archives from [GitHub Releases](https://github.com/WindustH/wish-core/releases). Once installed, run `wish` and open `http://127.0.0.1:8790` in your browser:

```sh
wish
```

## Documentation

- **User guides**: [Getting Started](docs/user/getting-started.md) · [Configuration](docs/user/configuration.md) · [Deployment](docs/user/deployment.md) · [HTTP API](docs/user/api.md)
- **Developer notes**: [Architecture & Internals](docs/developer/README.md)

The complete offline manual is embedded in the executable: run `wish docs`, `wish docs configuration`, or `wish docs architecture`. Search with `wish docs search <words>`. No server or network is required.

## License

[MIT](LICENSE)
