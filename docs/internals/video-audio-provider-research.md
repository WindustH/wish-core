# 视频与音频生成供应商调研及 Wish 接入方案

调研日期：2026-10-29。状态：**调研与设计，尚未实现视频/音频工具**。

本报告核对官方开发文档、官方模型目录和生命周期公告。没有调用付费接口，
没有测量真实生成质量、延迟、成功率或账号权限。优先级是基于 Wish 的已有架构、
文档可验证程度和接入成本作出的工程判断，不是模型质量排行榜。

## 1. 结论与分类

视频生成、配音、音乐和音效都有可接入的商业服务。建议用户界面分成「视频生成」
和「音频生成」两组，但能力契约区分四种用途：

| 能力 | 典型输入 | 典型输出 | 重要参数 |
| --- | --- | --- | --- |
| 视频 | 场景描述、图片；部分模型支持视频/音频参考 | MP4；可能自带对白、音乐和音效 | 时长、画幅、分辨率、首尾帧、是否生成声音 |
| 语音合成 / TTS | 要逐字朗读的文字、说话人 | WAV/MP3/Opus 等 | 音色、语言、语速、情感、多说话人 |
| 音乐 | 风格描述、可选歌词、段落安排 | 人声歌曲或纯音乐 | 时长、歌词、纯音乐、结构 |
| 音效 / SFX | 声音描述、可选视频 | 环境声、拟音、效果音 | 时长、循环、与画面同步 |

语音识别、翻译、去人声、分轨和实时语音聊天属于相邻能力，应单独建模。
例如音视频联合生成不等于该平台同时提供独立音乐 API。

## 2. 已发现的重要生命周期变化

| 项目 | 本次查到的状态 | 对 Wish 的影响 |
| --- | --- | --- |
| OpenAI Sora / Videos API | 官方列明 2026-09-24 停用，日期已经过去 | 不列入新增视频接口，也不能从 ChatGPT 生图通道推导出订阅生视频能力。[官方公告](https://developers.openai.com/api/docs/deprecations) |
| Gemini API Veo 3.1 Preview | 三个预览 ID 于 2026-10-22 关闭；官方引导到 Omni 或企业平台 GA 模型 | Gemini Developer API 优先研究 `gemini-omni-1.1-flash`；不能把这一公告泛化成所有渠道的 Veo 均不可用。[Veo 状态](https://ai.google.dev/gemini-api/docs/veo) |
| OpenAI 旧 TTS | `tts-1`、`tts-1-hd`、`gpt-4o-mini-tts` 将于 2027-01-06 停用 | 新接入需考虑 Realtime 工作流；不是把旧 `/audio/speech` 的 model 字符串替换就行。[TTS 文档](https://developers.openai.com/api/docs/guides/text-to-speech) |
| Luma | 旧 Dream Machine 文档引导到 Luma Agents API | 新适配器以 Agents 文档为基准，区分新旧鉴权、地址和响应。[入口迁移说明](https://docs.lumalabs.ai/docs/welcome)、[新 API](https://docs.agents.lumalabs.ai/) |
| Suno | 官方 `platform.suno.com` 已展示音乐生成 REST API 入口 | 不能沿用「Suno 没有官方 API」的旧说法；本次未登录，具体 schema、开放条件、计费和订阅关联仍待核实。[官方平台](https://platform.suno.com/) |
| Udio | 当前官方帮助页仍明确没有 public API | 不把第三方同名包装当成官方接口。[官方帮助](https://help.udio.com/en/articles/10756277-udio-public-api) |

搜索缓存、教程样例和当前公告可能不同步；默认模型要检查生命周期，不能仅凭
还能搜到一个示例就标记可用。供应商存在 API、具体模型开放、当前账户获准调用、
真实调用成功，是四个不同的验证层级。

## 3. 视频主要供应商

以下按接入渠道列出，部分供应商同时经营自研模型和第三方聚合服务。
同名模型经不同渠道调用时，地址、计费、输入字段和功能开放范围可能不同。

| 供应商 / 模型家族 | 已核实的能力与接入形态 | Wish 建议及限制 | 官方资料 |
| --- | --- | --- | --- |
| Google Gemini Omni；企业平台 Veo | Omni 文/图生视频、视频编辑与延长；Interactions API 可返回视频数据/URI | 首批候选；已有 Google 账户基础，但必须按新的 Interactions schema 适配 | [Omni](https://ai.google.dev/gemini-api/docs/omni)、[模型](https://ai.google.dev/gemini-api/docs/models/gemini-omni-flash) |
| 阿里云 Wan / 通义万相 | 文生、图生、参考生视频；异步任务；地区与 workspace 相关 | 首批候选；2.7 文档明确是新协议，不可认为 Wan3.0 只换 model 即兼容；后者需另核对 | [Wan2.7 接口](https://www.alibabacloud.com/help/en/model-studio/text-to-video-api-reference)、[家族目录](https://docs.modelstudio.console.alibabacloud.com/en/model-studio/wan-api-reference) |
| 字节 / 火山 Seedance | 多模态参考、生成、编辑、延长；任务接口 | 首批候选；Ark、LAS、海外代理渠道分开配置；开通条件需在账户核实 | [教程](https://docs.volcengine.com/docs/ark/seedance-2-0?lang=zh)、[通用教程](https://docs.volcengine.com/docs/82379/2298881?lang=en&redirect=1) |
| MiniMax H3 / 海螺 | H3 的 `content[]` 支持文/图/视频/音频参考；V2 异步提交、查询、取消或删除 | 首批候选；老 Hailuo V1 与 H3 V2 作为不同协议版本 | [创建任务](https://platform.minimax.io/docs/api-reference/video-generation-v2-create)、[总览](https://platform.minimax.io/docs/api-reference/api-overview) |
| 快手 Kling / 可灵 | 官方开发平台及独立 API 资源包；fal 等也提供 Kling 模型 | 主要候选；官方新快速入门的搜索索引显示 Bearer API Key，直接抓取只有 JS 外壳，逐接口 schema 尚未完成核验；不要照抄旧 AK/SK-JWT 教程 | [官方快速入门](https://kling.ai/document-api/guides/get-started/quick-start)、[fal 模型目录](https://fal.ai/explore) |
| Runway | 文生、图生、视频变换；统一异步 task 接口；同时列出自研与外部模型 | 第二批直连，或作为聚合渠道；请求要求版本 Header | [API reference](https://docs.dev.runwayml.com/api/) |
| Luma Ray | 新 Agents API 提交 generation、查询、下载；文/图生视频及编辑 | 第二批；采用新协议，旧 Ray 2 样例不作为新默认 | [Agents quickstart](https://docs.agents.lumalabs.ai/) |
| xAI Grok Imagine | 文/图生视频；`request_id` 查询后取得 `video.url`；有生成、编辑、延长相关能力 | 首批候选；复用 xAI API 账户，不推断 Grok 网页订阅可直接调用 | [视频接口](https://docs.x.ai/developers/model-capabilities/video/generation) |
| BFL FLUX 3 Video | 文生、图片关键帧、延长，支持同步音轨；`x-key` + polling URL | 首批候选；可复用 BFL 的账户及轮询处理，但输出改为视频 | [FLUX 3 Video](https://docs.bfl.ai/flux_3/flux3_video) |
| Lightricks LTX | 文/图/音频生视频；含同步声音；局部重做、延长、重新构图；异步 job | 第二批；适合补音频驱动画面和编辑能力 | [官方能力与 API](https://docs.ltx.io/welcome) |
| 生数 Vidu | 文/图/首尾帧/参考生视频；Q3 文档含音视频同步、任务查询和取消；另有 Q4 Preview | 第二批；官方 REST 已可核查；off-peak 任务可能等待很久，不能使用固定十分钟任务寿命 | [Q3 接口](https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/text-to-video) |
| PixVerse / 拍我 | 文/图生视频、首尾帧、延长、口型等；API-KEY、请求追踪 ID、异步 video_id | 第二批；中国与国际站端点分开；必须同时检查 HTTP 与业务 ErrCode | [国际接口入门](https://docs.platform.pixverse.ai/start-here-2394989m0)、[中国入口](https://docs.platform.pai.video/9405333m0) |
| Pika | 自有 Pika 视频与效果能力；新的 Pika API 也聚合视频、语音、音乐等外部模型 | 作为第二个聚合平台候选；商业订阅/平台会员/生成用量要分清 | [官方平台](https://dev.pika.art/)、[开发者入口](https://dev.pika.art/agent) |
| Adobe Firefly | 官方 Generate Video API 文档及视频尺寸、输入存储规则 | 企业工作流候选；还需核实具体账户的产品开通及授权流程 | [视频 API 使用说明](https://developer.adobe.com/firefly-services/docs/firefly-api/getting-started/usage-notes/) |
| 腾讯云 Hunyuan / MPS | MPS 提供 CreateAigcVideoTask，模型包含 Hunyuan 及外部供应商 | 云账户用户的补充渠道；与厂商直连协议分开，不能把 MPS 字段套给混元原生接口 | [MPS API](https://cloud.tencent.com/document/api/862/126965) |

Amazon Nova Reel 也有官方异步生成与 S3 输出文档，但查到的模型卡标有 Legacy/EOL 信息。
没有逐版本核实可用性前，不设为新项目默认；这不等于宣称所有 Nova 版本都已关闭。
参考：[调用流程](https://docs.aws.amazon.com/nova/latest/userguide/video-gen-access.html)、
[模型卡](https://docs.aws.amazon.com/bedrock/latest/userguide/model-card-amazon-nova-reel.html)。

## 4. 语音合成主要供应商

| 供应商 | 适合纳入的能力 | 协议与接入考虑 | 官方资料 |
| --- | --- | --- | --- |
| ElevenLabs | 配音、多语种、角色对话、声音克隆 | 专门 TTS/Dialogue API，音色需要按账户列出；同时还有音乐和音效接口 | [TTS](https://elevenlabs.io/docs/overview/capabilities/text-to-speech) |
| MiniMax Speech | 中外文配音、风格/语速、声音克隆、长文本 | HTTP / WebSocket / 异步长音频；HTTP 可返回 hex 音频，不能只实现 base64 解码 | [HTTP TTS](https://platform.minimax.io/docs/api-reference/speech-t2a-http)、[长音频](https://platform.minimax.io/docs/guides/speech-t2a-async) |
| 阿里 Qwen-TTS / CosyVoice | 配音、角色音色、复刻 | Qwen HTTP 与 CosyVoice/Realtime 的接口应分别核验；有地区与 workspace 绑定 | [Qwen-TTS](https://www.alibabacloud.com/help/en/model-studio/qwen-tts-api)、[声音管理](https://www.alibabacloud.com/help/en/model-studio/voice-clone-design-http-api) |
| 豆包语音 / Seed TTS | 中文配音、音色/方言、长文本、声音复刻 | HTTP Chunked、SSE、WebSocket、异步任务都存在；语音账户字段不能直接假定等于 Ark 模型 Key | [接口选择](https://docs.volcengine.com/docs/doubao-voice/websocket-unidirectional-streaming-v3?lang=zh)、[长文本](https://docs.volcengine.com/docs/doubao-voice/task-sub-mission?lang=zh) |
| Google Gemini TTS | 单/多说话人、可控朗读、声音设计/复刻 | 当前指南使用 Gemini 3.8 TTS 与 Interactions；旧模型使用不同请求形式；区分 WAV 与原始 PCM | [当前 TTS 指南](https://ai.google.dev/gemini-api/docs/speech-generation) |
| Fish Audio | 配音、多说话人、实时输出、声音克隆 | REST/流式；声音 ID 与账户相关；网页音乐/音效入口不代表这两类已经有公开 API | [官方能力与边界](https://docs.fish.audio/overview/capabilities) |
| Cartesia Sonic | 配音、流式语音、克隆 | REST / WebSocket；低延迟语音场景的候选，宣传延迟不作为 Wish 的实测指标 | [官方文档](https://docs.cartesia.ai/get-started/overview) |
| Deepgram Aura | REST/流式 TTS | Token 鉴权，返回音频流；应按音色目录核实语言，不预设中文覆盖 | [快速入门](https://developers.deepgram.com/docs/text-to-speech) |
| Microsoft Azure Speech | 企业配音、多语言、SSML、区域音色目录 | 资源 Key/区域或 Entra；SSML 请求，二进制音频响应 | [REST API](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech) |
| Amazon Polly | 多语言朗读、SSML、长文本任务 | AWS 签名/权限，流式 AudioStream；长任务采用不同接口 | [SynthesizeSpeech](https://docs.aws.amazon.com/polly/latest/APIReference/API_SynthesizeSpeech.html) |
| OpenAI | TTS / Realtime 音频输出 | 旧 Speech API 已有停用日；新接入应设计 Realtime 适配；实时生成与严格逐字朗读要实际评估 | [TTS 与迁移说明](https://developers.openai.com/api/docs/guides/text-to-speech) |
| 硅基流动 / Together | 托管多种 TTS 模型 | 可利用较熟悉的 speech API 外观，但不同模型的音色、多说话人、参考音频字段仍不同 | [硅基流动](https://docs.siliconflow.cn/docs/api/audio-speech-post)、[Together](https://docs.together.ai/docs/inference/text-to-speech/overview) |

Vidu 也有独立[语音接口](https://platform.vidu.com/docs/api-reference/audio-generation/text-to-speech)，
Pika 聚合目录列出 Speech；属于已有视频账户用户的可选补充。
传统语音云厂商并不止上表这些，本次优先覆盖生成式媒体平台与常见云端接入。

## 5. 音乐和音效主要供应商

| 供应商 | 本次核实结果 | 建议 | 官方资料 |
| --- | --- | --- | --- |
| ElevenLabs Music / Sound Effects | 分别有 `/v1/music` 与 `/v1/sound-generation`；音乐可用 prompt 或 composition plan，支持纯音乐约束 | 首批：一个账户覆盖语音、音乐和音效，但三个能力各用自己的 schema | [音乐](https://elevenlabs.io/docs/api-reference/music/compose)、[音效](https://elevenlabs.io/docs/api-reference/text-to-sound-effects/convert) |
| MiniMax Music | `music-3.0` 官方 API，prompt、lyrics、audio_setting 等 | 首批：歌曲及歌词驱动生成 | [API](https://platform.minimax.io/docs/api-reference/music-generation) |
| Google Lyria | Lyria 3.5 与 Clip；Interactions 返回音频、歌词/结构；另有 Lyria RealTime | 首批：先实现文件型歌曲，实时连续音乐单列后续能力 | [音乐指南](https://ai.google.dev/gemini-api/docs/music-generation) |
| Stability Stable Audio | 当前文档含 Stable Audio 3.0 异步结果接口；支持音频生成与转换 | 第二批：补音乐/声音设计；需要持久化任务和结果下载 | [API 与结果机制](https://docs.stability.ai/api-reference)、[发布说明](https://platform.stability.ai/docs/release-notes) |
| Suno | 官方 API 平台存在，可生成歌曲、covers、mashups；公开页面为登录入口 | 重点候选，登录后核实 schema/计费/权限再编码；不把第三方 Suno API 当官方 | [官方平台](https://platform.suno.com/) |
| Udio | 官方仍声明无 public API | 暂不做官方适配器，保留人工导入生成文件 | [说明](https://help.udio.com/en/articles/10756277-udio-public-api) |
| Mubert | 官方音乐生成、流式音乐与曲库 API 产品 | 按背景配乐需求补充，先核实申请/套餐与授权范围 | [API 产品](https://mubert.com/api) |
| SOUNDRAW | 官方音乐 API；有 API 订阅计划和商用授权说明 | 背景音乐候选；平台 API 套餐与普通创作订阅分开确认 | [API 产品与套餐](https://soundraw.io/api) |
| Beatoven.ai | 官方展示音乐和 SFX API 集成 | 背景配乐/音效候选；取得具体开发文档和准入信息后接入 | [API 产品](https://www.beatoven.ai/api) |
| Vidu / Pika | Vidu 有 Text-to-Audio 与可控声音接口；Pika 目录列出 Music/SFX/Soundtrack | 可复用视频平台账户，但每种功能单独验证 | [Vidu 音频](https://platform.vidu.com/docs/api-reference/audio-generation/text-to-audio)、[Pika 目录](https://dev.pika.art/) |

有歌词歌曲、纯音乐、循环背景声、给既有视频自动配乐/拟音，不能视为同一种输入。
参考音频也可能用于续写、风格参考或声音克隆，必须明确用途，避免静默改变行为。

## 6. 聚合平台与直连的取舍

| 渠道 | 优势 | 必须保留的区别 |
| --- | --- | --- |
| [fal](https://fal.ai/explore) | 多视频/音频模型，已有 Wish 账户与 queue 接入经验 | 每个模型有独立输入 schema；共享队列协议不等于支持任意模型 |
| [Replicate](https://replicate.com/docs/topics/predictions/create-a-prediction) | 统一 prediction 生命周期，覆盖官方及社区模型 | 官方模型与社区版本的创建接口、输出形状、授权不同；必须固定经过核验的预设 |
| [Runway](https://docs.dev.runwayml.com/api/) | 自研模型和外部模型共用 task 外层 | 不同 model 的请求字段与支持能力不同 |
| [Pika API](https://dev.pika.art/) | 视频、图片、音频/音乐的多模型目录 | 会员费、平台费、单次生成费用需分别记录；不采用广告中的折扣作稳定成本 |
| [硅基流动](https://docs.siliconflow.cn/docs/api/audio-speech-post) / [Together](https://docs.together.ai/docs/inference/text-to-speech/overview) | 已有账户体系，可补托管 TTS | 不把 OpenAI 风格端点当成完整协议兼容声明 |

建议组合是「一个主要聚合平台 + 几个常用原厂直连」。本次推荐先用 fal 扩覆盖，
原厂优先考虑 MiniMax、Google、阿里、火山、ElevenLabs、xAI/BFL。
这基于复用与覆盖考虑，不表示 fal 一定比原厂便宜或快。

## 7. 费用、订阅与地区

不应统一按「生成一次」比较价格。Wish 的费用记录至少需要：供应商、渠道、模型版本、
输入/输出媒体时长、分辨率、生成声音选项、实际用量单位和币种；未知费用保留未知。

| 费用类型 | 应如何理解 | 已核实示例 |
| --- | --- | --- |
| 视频 | 秒数、分辨率、模型档位、输入视频及音轨都可能影响费用 | [xAI](https://docs.x.ai/developers/model-capabilities/imagine)、[MiniMax](https://platform.minimax.io/docs/pricing/overview) |
| TTS | 字符、token 或生成时长；音色/模型/套餐影响单价 | [ElevenAPI](https://elevenlabs.io/pricing/api)、[Gemini](https://ai.google.dev/gemini-api/docs/pricing) |
| 音乐 | 生成次数、输出时长、credits 或订阅额度 | [MiniMax](https://platform.minimax.io/docs/pricing/overview)、[SOUNDRAW](https://soundraw.io/api) |
| 聚合 | 模型用量可能之外另有会员或平台费用 | [Pika](https://dev.pika.art/) |

本次没有在同一输入、同一时长和同一质量档位下实测，因此不列「最便宜」排名。
文档价不能替代账户报价；营销优惠也不能当长期成本。

账户统一只代表 Wish 共用凭据管理，不代表网页会员、Coding Plan、API Key、
媒体资源包天然互通。例如 MiniMax 官方明确区分 M Plan Subscription Key 与按量 API Key；
PixVerse 官方文档也区分 API credits 和网页会员。应通过能力与计费渠道元数据表达，
而不是因为同品牌就让任意凭据出现在所有服务选择器中。
参考：[MiniMax 鉴权](https://platform.minimax.io/docs/api-reference/api-overview)、
[PixVerse API 说明](https://docs.platform.pixverse.ai/pixverse-api-llm-txt-2109771m0)。

ChatGPT/Codex 生图通道的存在不能推导出语音或视频的订阅调用权。本次没有核实到
可直接复用当前 Wish ChatGPT 登录凭据的官方音频文件生成契约；这一项应标为未确认。
Suno 官方 API 与其创作订阅的额度关系同样需要登录后核实，不能直接承诺免费复用。

## 8. Wish 当前基础与需要补齐的部分

本节来自本地源码审阅，不是供应商文档陈述。

可复用部分：账户注册表、服务配置与能力开关、工具执行/取消链路、会话 blob 目录、
工具结果中记录 blob ID/本地路径，以及前端工具详情入口。

目前 `application/image_generation.rs` 的下载返回 `Vec<u8>`，每图限制 32 MiB，
一次任务总时限 600 秒。`storage/blobs.rs::save_upload` 接受整个字节切片，
MIME 检测只识别受支持的图片；读 blob 也一次返回完整 `Vec<u8>`。
这些设置适用于现有生图范围，不能直接扩大常量后作为视频系统。
底层 transport 已有分块读取接口，可以用于流式文件写入，不必另起一套网络栈。

建议分层：

```text
工具：video_generate / speech_generate / music_generate / sound_effect_generate
  ↓
应用：生成任务、账户选择、并发/费用约束、恢复和取消
  ↓                         ↓
协议：厂商请求/响应映射       媒体产物：流式下载、校验、blob 登记
  ↓                         ↓
现有 transport             storage
```

这是建议的新边界，不是已实现模块。视频与音频可以共用任务与产物基础设施，
但模型特有参数需要 typed request/capabilities，不能全部塞入无校验 JSON。

### 8.1 可恢复的后台任务

视频/长音乐生成独立于某次聊天 HTTP 请求。建议本地任务记录至少保存 owner、
能力类型、供应商/模型、参数摘要、上游 task ID、状态、结果、费用信息和时间戳。
凭据仅保存账户引用，不把 token 写入任务、日志或模型上下文。

状态可分为 queued → submitting → running → downloading → succeeded，
另外表达 failed / cancelled / outcome_unknown / download_failed。
上游已经完成但本地下载失败时，应允许重新下载该产物，不能重新付费生成。
后台任务仍须受应用生命周期管理，并在重启后恢复查询；会话删除需有明确的任务
取消/归属和待落盘文件清理规则。

提交响应丢失时可能不知道是否扣费。只有供应商明确支持且经过验证的幂等键，
才允许相同键重发。追踪 ID 不自动等于幂等键。轮询和下载重试与重新提交付费任务
应是三个不同操作。取消等待也不等于供应商取消或退款。

### 8.2 先落盘，再把本地路径交给模型

供应商字节流 → 临时文件 + 增量 SHA-256 → 校验/元数据 → 原子发布到 blob →
持久化工具产物 → 返回模型。下载成功后才把产物标记为可用。

返回字段建议含 blob ID、path、MIME、字节数、时长、视频尺寸/音轨、供应商和模型。
附带的封面、字幕、歌词或分轨也是独立产物，不把几百 MB 的 base64 塞回上下文。
模型得到路径不代表它已经看过视频或听过音频，理解能力应另走模型输入或分析工具。

### 8.3 播放与参考素材

前端用原生 audio/video 播放器配合后端 Range/206、HEAD、Content-Length 等按需读取，
避免为了播放开头先下载完整文件。授权方式必须支持媒体元素的分段请求，不能把 API
账户 Key 放进播放器 URL。需要 MP4/MOV/WebM、MP3/WAV/Ogg 等 MIME/容器识别与
大小/时长限额；转封装、缩略图与探测走受限任务，不能阻塞 HTTP worker。

视频参考素材优先接收 Wish blob ID。供应商需要公开 URL 时，优先使用它的上传 API
或受控短期签名资源；localhost 与局域网地址不能直接交给云端。不同端点可能需要
带认证下载或无凭据 CDN 下载，应按明确的主机/用途规则选择，重定向不继承秘密。

## 9. 推荐落地顺序

1. **共用基础设施**：后台生成任务持久化、流式 blob 写入、媒体元数据、Range 播放，
   以及生成卡片的进度/取消/继续查询/重新下载。现有生图随后复用这一层。
2. **最小可验证覆盖**：视频先选一个轮询型通道（MiniMax H3 或 Wan）与 fal；
   音频先 ElevenLabs 的配音/音效、MiniMax Music，验证所有产物路径。
3. **首批扩展**：Google Omni/TTS/Lyria、Seedance、xAI/BFL，以及 MiniMax/Qwen TTS。
   Kling 官方需要先补齐动态文档/账号验证；其聚合版本可独立推进。
4. **第二批**：Runway、Luma、LTX、Vidu、PixVerse、Stability、Fish、Cartesia，
   以及 Replicate/Together/硅基流动按实际需求补齐。
5. **受准入影响或专项**：Suno 登录后的官方 API、企业云媒体流程、
   Mubert/SOUNDRAW/Beatoven、长文本/分轨、视频编辑与实时音频。

首轮用已有外部集成测试验证协议、任务恢复、断线不重复提交、文件下载和鉴权；
再由有额度的账号逐家做最小付费验证，记录最终模型/地区/产物。模拟通过与真实
服务验证分开标记，不新增用户明确不希望保留的单元测试体系。

本报告只新增调研文档；上述视频与音频功能没有被部署或宣称已支持。
