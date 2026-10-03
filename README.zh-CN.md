<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/wish-logo-dark.svg">
    <img src="docs/assets/wish-logo-light.svg" alt="Wish" width="300">
  </picture>
</p>

<p align="center">
  <strong>运行在你自己机器上、什么都记得的 AI 智能体。</strong>
</p>

<p align="center">
  <a href="https://github.com/WindustH/wish-web">Web 应用</a> ·
  <a href="docs/README.md">文档</a> ·
  <a href="docs/api.md">HTTP API</a> ·
  <a href="README.md">English</a>
</p>

---

Wish 在你掌控的电脑上运行长期存在的 AI 智能体会话。为会话指定一个工作目录和一个模型，
它就能和你对话、执行命令、查看图片，并持续推进长时间的任务；期间交流的每一个字都会被保存下来，随时可以检索。
你可以在桌面或手机上通过 [Wish Web 应用](https://github.com/WindustH/wish-web) 使用它，
也可以用任何能发 HTTP 请求的工具直接调用。

<p align="center">
  <img src="docs/assets/screenshot-desktop-zh.png" alt="桌面端的 Wish" width="74%">
  &nbsp;
  <img src="docs/assets/screenshot-mobile-zh.png" alt="手机上的 Wish" width="22%">
</p>

## 为什么选择 Wish

- **一个小巧的程序。** 只有一个 `wish` 可执行文件，自带嵌入式数据库。无需安装数据库服务、容器或额外运行时。
- **支持你正在用的模型。** 内置 48 个预设，覆盖 OpenAI、Anthropic、Google Gemini、AWS Bedrock、
  DeepSeek、通义千问、Kimi、智谱 / Z.ai、MiniMax、Mistral、xAI、OpenRouter 等，
  也支持通过 Ollama、LM Studio、vLLM 使用本地模型。每家服务都用它原生的协议对接，包括推理（思考）内容。
  还可以直接用 ChatGPT 账号登录。
- **随时改主意。** 任何时候都能切换提供商或模型，即使智能体正在工作；新的选择从它的下一步开始生效。
- **什么都不会丢。** 每条消息、每个事件都会被永久保存。可以用关键词检索会话的全部历史（中文同样适用），
  智能体自己也能检索过去的内容。
- **长对话依然好用。** 上下文变大时，Wish 会自动压缩：用滚动生成的摘要，或者交给提供商自己的压缩能力。
  完整的原始历史始终保留。
- **能干活的 Shell。** 智能体在会话目录中执行命令，把耗时任务放到后台并在完成时收到通知，
  可以向交互式程序输入内容，并准确报告每次文件修改改了什么。
- **接入 MCP 不增加负担。** 添加本地或远程的 MCP 服务器后，智能体在 Shell 里调用它们，每个会话各用一份实例。
  不管接入多少个，它们的工具都不会挤占模型的上下文，也不会让提示缓存失效。
- **Skill 按需加载。** 放进 Skill 文件夹即可，格式就是其他 agent 也在用的 `SKILL.md`。
  智能体会在任务需要时自己查找并读取对应的 Skill，在此之前它们不占用任何上下文。
- **需要你拿主意时会问你。** 智能体可以用一张表单向你提出选择题和填空题；即使它等不及先继续了，
  你之后补充的回答也会送到它那里。
- **它工作时你也能继续说。** 追加的消息会排队，可以调整顺序或取消；随时可以中断；
  也可以问一个“顺便问一下”的小问题，而不打扰正在进行的任务。
- **稳定可靠。** 关掉浏览器任务也会继续运行。关闭服务时会保留已生成的部分回答；
  崩溃之后，Wish 绝不会自作主张地重复执行命令。
- **用量一目了然。** 按模型统计 Token 用量、缓存命中、估算的流式速度，以及每日活跃日历。
- **改配置不用重启。** 在 Web 应用里添加提供商、更换模型、设置代理或 Shell，立即生效。

## 快速开始

**1. 安装 Wish**，用你习惯的包管理器即可：

```sh
npm install -g wish-agent               # Linux、macOS 和 Windows
yay -S wish-agent-bin                   # Arch Linux
brew install windusth/tap/wish-agent    # macOS 和 Linux
```

**2. 启动。**

```sh
wish-agent
```

**3. 打开 <http://127.0.0.1:8790>。** 首次使用会有一个简短的引导，帮你添加模型提供商。
然后在首页选好工作目录，发送第一条消息即可。

首次启动会为当前用户写一份配置文件（Linux 上是 `~/.config/wish-agent/config.json`，
macOS 上在 `~/Library/Application Support/wish-agent/`，Windows 上在 `%APPDATA%\wish-agent\`），
数据也保存在它旁边。你添加的提供商会保存在这份配置里。如果不想把 API 密钥写进文件，
可以在启动 Wish 前把它导出为环境变量，然后在 Web 应用中填写 `${变量名}`。
全部选项见[配置说明](docs/configuration.md)；以服务方式运行、设置访问令牌、
从其他设备访问，见[部署指南](docs/deployment.md)。

### 从源码构建

需要 [Rust 工具链](https://rustup.rs)（stable）和 [Node.js](https://nodejs.org) 22.19 或更高版本。

```sh
git clone https://github.com/WindustH/wish-web.git
(cd wish-web && ./pnpmw install --frozen-lockfile && ./pnpmw build)
git clone https://github.com/WindustH/wish-core.git
cd wish-core
cargo build --release
cp -r ../wish-web/dist target/release/web
./target/release/wish
```

### 不使用 Web 应用

Web 应用能做的一切都可以通过 [HTTP API](docs/api.md) 完成：

```sh
# 在 /tmp 中创建一个带 Shell 的会话
curl -s http://127.0.0.1:8790/api/sessions -H 'Content-Type: application/json' -d '{
  "provider": "openai", "cwd": "/tmp", "tools": {"shell": true},
  "config": {"model": "gpt-5", "stream": true, "tools": [], "run": {"tools": "Serial"}}}'

# 发送一条消息，它会立即开始工作
curl -s http://127.0.0.1:8790/api/sessions/SESSION_ID/input \
  -H 'Content-Type: application/json' -d '{"text": "这个目录里有什么？"}'

# 实时查看进展
curl -N http://127.0.0.1:8790/api/sessions/SESSION_ID/events
```

## 文档

文档目前为英文。

| | |
| --- | --- |
| [配置](docs/configuration.md) | `config.json` 的全部选项、提供商与预设 |
| [部署](docs/deployment.md) | 以服务方式运行、访问控制、远程访问、备份与升级 |
| [HTTP API](docs/api.md) | 接口、事件流与错误处理 |
| [内部实现](docs/internals/README.md) | 引擎的工作原理，面向贡献者 |

## 安全

Wish 是为单个受信任的使用者设计的。任何能访问它 API 的人，都能以 Wish 运行账户的权限执行命令。
默认只监听 `127.0.0.1`；没有设置令牌时，它只回应用它自己的地址发来的请求，
网页无法借你的浏览器操纵它。在对外开放之前，请设置访问令牌并通过 HTTPS 提供服务，
具体做法见[部署指南](docs/deployment.md)。

## 参与贡献

欢迎提交 Issue 和 Pull Request。Wish 使用 Rust（edition 2024）编写，`cargo build` 即可构建。
测试套件位于独立的 `wish-test` 仓库，通过 HTTP 驱动真实的程序并连接本地模拟的提供商，不需要 API 密钥，也不需要联网。

## 许可证

[MIT](LICENSE)
