<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/wish-logo-dark.svg">
    <img src="docs/assets/wish-logo-light.svg" alt="Wish" width="300">
  </picture>
</p>

<p align="center">
  <a href="docs/user/README.md">用户文档</a> ·
  <a href="docs/developer/README.md">开发者文档</a> ·
  <a href="https://github.com/WindustH/wish-web">Wish Web</a> ·
  <a href="README.md">English</a>
</p>

---

Wish 是一个简洁、可靠、功能完备的 AI Agent Harness。它以单个 Rust 程序搭配内嵌数据库运行在你自己的机器上，配合为桌面端和移动端精心打磨的 [Wish Web](https://github.com/WindustH/wish-web)，无论用电脑还是手机都能轻松接入。希望你的愿望能通过 Wish 实现！

## 下载与使用

安装 `wish-agent`（已内置编译好的前端页面）：

```sh
npm install -g wish-agent               # Linux、macOS 和 Windows
yay -S wish-agent-bin                   # Arch Linux
brew install windusth/tap/wish-agent    # macOS 和 Linux
```

也可以前往 [GitHub Releases](https://github.com/WindustH/wish-core/releases) 下载对应平台的压缩包。装好后启动服务，浏览器打开 `http://127.0.0.1:8790` 即可：

```sh
wish
```

## 文档

- **用户文档**（英文）：[快速上手](docs/user/getting-started.md) · [配置说明](docs/user/configuration.md) · [部署指南](docs/user/deployment.md) · [HTTP API](docs/user/api.md)
- **开发者文档**（英文）：[架构与实现](docs/developer/README.md)

## 许可协议

[MIT](LICENSE)
