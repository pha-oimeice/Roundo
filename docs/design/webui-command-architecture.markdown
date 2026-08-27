# Roundo Web UI 与 Client Command 架构设计

> **受众**：负责实现、审查或继续设计本功能的 AI/工程师。  
> **状态**：设计已确认，尚不能据此宣称代码已经实现。  
> **规范性**：本文是本次 Web UI/命令重构的详细实施依据；领域术语以根目录 `CONTEXT.md` 为准，关键取舍另见 `docs/adr/0001-typed-json-client-commands.markdown` 与 `docs/adr/0002-mod-registered-wry-ui.markdown`。

## 1. 背景与问题

当前 `roundo_client/src/ui` 使用 Bevy 原生 UI。它不仅负责视觉展示，还直接拥有或操作客户端 UI phase、连接管理、服务器探测、服务器配置持久化、设置修改、按键绑定、退出逻辑和鼠标模式。这使视觉实现与客户端能力严重耦合，无法由 Mod 替换，也无法独立开发。

当前仓库中的相关事实：

- `roundo_client/roundo_webui` 已存在，但实现为空；Wry 依赖已经声明。
- `game/common/roundo_mod_loader` 仍是模板，尚无可用加载路径。
- `mods/vanilla_ui` 当前只有 manifest，没有 Web UI project。
- `roundo_cli` 当前主要是 stdin Adapter，命令 enum 私有，Client/Server 直接打印文本。
- `ClientConfig`、`ServerEntry` 与部分持久化逻辑位于客户端二进制 crate；`ServerConfig` 位于服务端二进制 crate。
- 现有网络连接并非客户端启动时自动建立，而是由当前 UI 发起；网络层已经维护连接、重连和连接状态。

目标不是简单地把 Bevy Node 改写成 HTML，而是建立两个稳定 seam：

1. **Client Command seam**：所有调用方用同一套类型化 JSON 指令调用客户端能力，并得到类型化 JSON 结果。
2. **UI Resource seam**：客户端通过全局 slot 打开由 Mod 注册的 Web UI Project，而不拥有 Project 内部视觉状态机。

## 2. 目标

- 用 Wry child WebView 在 Bevy 窗口之上渲染 Web UI。
- 将现有 S0～S5 全部转译为 Mod 提供的 HTML/CSS/JS UI Resource。
- 删除旧 Bevy UI 运行路径，不维护双实现 fallback。
- 把客户端能力收敛为 `roundo_cli` 拥有的 Client Command definitions/handlers。
- 允许终端、Web UI、Bot、外挂或其他 Adapter 调用同一命令接口；命令执行语义不取决于调用方身份。
- 让 UI 自己维护页面状态、选择状态、折叠状态及导航流程。
- 让 Mod 可以通过全局 UI Registry Slot 替换某类 UI，而无需冒充另一个 Mod 的资源身份。
- Client 与 Server 配置都新增无行为的 `dev_mode = false`。
- 首期完成 Windows 集成；其他平台不提供未经验证的实现。

## 3. 非目标

- 不实现完整的未来数据驱动输入模块；本期仅保留 Escape 打开暂停 UI 的最小 Adapter。
- 不设计物品栏等尚不存在的 UI。
- 不实现权限沙箱；所有已安装 Mod 被完全信任，所有调用方都可以尝试执行任何命令。
- 不让 `dev` 等级成为授权机制。
- 不实现 shell 管道、重定向、变量展开、命令替换或完整 shell。
- 不完成所有类型 Mod Resource 的生命周期；只实现 Web UI 所需的通用候选解析、manifest/依赖验证和 UI Registry 加载。
- 不在无桌面 CI 中启动真实 WebView2。
- 不自动解决 release 打包；运行时一律从当前工作目录下的 `mods/` 读取。

## 4. 规范词汇与不变量

### 4.1 Client Command

Client Command 是 versioned typed JSON。它表达客户端进程级能力，不等同于现有带 sequence 的 `Controller Command`。

不变量：

- Client Command 的执行语义与来源无关。
- 核心命令层只接收 JSON，不接收 Unix 文本。
- 每个请求必须返回 JSON；读取命令的返回值与命令本身同等重要。
- 输入/输出 schema 的唯一真值是客户端命令目录中的 Rust 类型。
- 网络请求类命令不等待网络往返；读取类命令只读取此前缓存的状态。

### 4.2 Client Interaction Mode

只有两个互斥硬件输入模式：

- `WebUi`：WebView 接管键鼠，鼠标释放并显示，游戏控制器输入关闭。
- `InGame`：Bevy 接管键鼠，鼠标锁定并隐藏，WebView 保持渲染但点击穿透。

这与 3D 世界是否可见是两个独立维度。

### 4.3 UI Resource、UI Asset 与 UI Registry Slot

- **UI Resource**：Mod 注册的一个 Web UI Project。
- **UI Asset**：UI Resource project root 内不单独注册的 CSS、JS、字体、图片等文件。
- **UI Resource Name**：运行时拼接出的可追踪全名：`<author>.<mod-name>.<resource-name>`。
- **UI Registry Slot**：全局、互斥、可被高优先级 Mod 覆盖的位置，值为一个 UI Resource Name。
- **Initial UI Slot**：客户端启动时解析并打开的 UI Registry Slot；它不是特殊类别，只是启动入口使用的全局互斥 slot。

资源身份与 slot 必须分离。Mod 不能通过伪造别人的 author/mod 名称替换资源；它只能让某个 slot 指向自己的资源。

## 5. 目标模块与依赖方向

```text
roundo_proc_macros
    └─ UnixCommand derive 及未来统一 proc macros

roundo_toolbox
    └─ bounded RequestResponsePipe<Request, Response>

roundo_mod_loader
    ├─ Mod manifest
    ├─ dependency graph/closure validation
    └─ generic priority candidate resolution

roundo_webui
    ├─ UI registry importer/resolver
    ├─ OpenUiRequest seam
    ├─ Wry/Windows adapter
    └─ JS IPC adapter

roundo_cli
    ├─ ClientCommandDefinition / UnixCommand traits
    ├─ Client/Server command registries
    ├─ command dispatcher and JSON schema
    ├─ terminal Adapter
    ├─ client handlers/network manager/probe manager
    └─ UI command handlers via roundo_webui interface

roundo_user_config
    ├─ ClientConfig / ServerConfig / ServerEntry
    └─ load/save/migration

roundo_client
    ├─ composition root
    ├─ 3D camera and targeting data production
    └─ installs roundo_cli + roundo_webui
```

关键依赖约束：

- `roundo_webui` 不依赖 `roundo_cli`，否则 `roundo_cli` 的 `ui.open` handler 会形成依赖环。
- 通用 request/response 机制位于 `roundo_toolbox`。
- `roundo_webui` 只接收 JSON request/response endpoint。
- `roundo_cli` 可以依赖 `roundo_webui` 暴露的控制 seam。
- `roundo_client` 只做进程组合，不再拥有 UI 流程状态机。

## 6. Client Command JSON 协议

### 6.1 输入 envelope

```json
{
  "version": 1,
  "command": "server.connect",
  "arguments": {
    "index": 0
  }
}
```

要求：

- `version` 必须存在，首期只接受 `1`。
- `command` 必须是已注册名称。
- `arguments` 必须是 JSON object；无参数命令也使用 `{}`。
- arguments 使用严格反序列化并拒绝未知字段。

### 6.2 成功结果

```json
{
  "version": 1,
  "command": "server.connect",
  "ok": true,
  "data": {
    "status": "connecting"
  }
}
```

### 6.3 错误结果

```json
{
  "version": 1,
  "command": "server.connect",
  "ok": false,
  "error": {
    "code": "index_out_of_range",
    "message": "server index 4 does not exist"
  }
}
```

至少定义以下框架错误 code：

- `unsupported_command_version`
- `unknown_command`
- `invalid_command_envelope`
- `invalid_arguments`
- `command_queue_full`
- `command_input_too_large`
- `internal_command_error`

具体 handler 可以定义稳定业务错误 code。不要要求 UI 解析人类 message 才能分支。

## 7. Command Definition 与 schema

规范性形状：

```rust
pub trait ClientCommandDefinition {
    type Input: serde::de::DeserializeOwned + schemars::JsonSchema;
    type Output: serde::Serialize + schemars::JsonSchema;

    const NAME: &'static str;
    const DEV_LEVEL: u8;
}
```

为使 `UnixCommand` derive 能看到字段，常规约定是 definition 自身也是 Input：

```rust
#[derive(Deserialize, JsonSchema, UnixCommand)]
#[serde(deny_unknown_fields)]
#[command(
    name = "server.connect",
    output = ConnectServerOutput,
    dev_level = 0
)]
#[unix(path = "server connect")]
pub struct ConnectServer {
    #[unix(positional)]
    pub index: usize,
}

impl ClientCommandDefinition for ConnectServer {
    type Input = Self;
    type Output = ConnectServerOutput;
    const NAME: &'static str = "server.connect";
    const DEV_LEVEL: u8 = 0;
}
```

Registry 显式注册 definition 与 handler：

```rust
registry.register::<ConnectServer>(handle_connect);
registry.register_unix::<ConnectServer>();
```

`command.schema` 必须从关联 Input/Output Rust 类型生成正式 JSON Schema，而不是返回手写示例。使用 JSON Schema 2020-12 和 `schemars`。schema 输出同时描述输入 envelope、成功 data 和公共错误 envelope。

## 8. Unix Projection

### 8.1 原则

Unix 文本只是一个输入 Adapter。客户端永远不执行 Unix 文本；Adapter 将其转成与其他调用方相同的 typed JSON。

只有显式实现 `UnixCommand` 的命令才有 Unix 表达。trait 与 derive macro 同名，分别位于类型和宏命名空间。宏统一位于新 workspace crate `roundo_proc_macros`，并由 `roundo_cli` 重导出。

```rust
pub trait UnixCommand: ClientCommandDefinition {
    const UNIX_PATH: &'static [&'static str];
    fn parse_unix(arguments: &[String])
        -> Result<serde_json::Value, UnixCommandParseError>;
}
```

### 8.2 derive 属性

```rust
#[unix(positional)]
index: usize,

#[unix(long = "https-addr")]
https_addr: String,

#[unix(short = 'v', long)]
verbose: bool,

#[unix(long, default = "250")]
interval_ms: u64,

#[unix(long)]
filter: Option<String>,
```

规则：

- positional 按声明顺序。
- bool option 不接值。
- `Option<T>` 可省略。
- `Vec<T>` 可重复。
- 其他字段必须提供或声明 default。
- 自动生成 usage。
- shell quoting/tokenization 在调用 trait 前完成。
- 不支持 shell 操作符和展开。

Registry 在启动时建立 Unix token path 的直接索引（HashMap 或前缀 trie）。禁止每次输入后遍历全部命令寻找匹配项。

终端别名可以映射到同一个 JSON command，不能为别名复制 JSON schema。例如 `server cancel` 只投影到 `server.disconnect`。

`export json <unix command...>` 是 Unix Adapter 对 `command.schema` 的快捷投影，不是客户端额外接受的文本协议。

## 9. Dev Level

`dev` 是进程内全局、非持久化的命令发现等级：

- 无参数：返回当前 level。
- 有参数：接受 `0..=3` 并修改当前 level。
- 仅过滤无指定命令的 `command.help` 输出。
- 不限制直接 help、schema 查询或命令执行。
- 与配置中的 `dev_mode: bool` 完全无关。

等级：

- Level 0：普通用户命令。
- Level 1：schema、position、HUD 等只读诊断。
- Level 2：显式修改 interaction mode 等调试命令。
- Level 3：预留给不稳定实验命令。

## 10. Request/Response I/O

`roundo_toolbox` 新增泛型 bounded `RequestResponsePipe<Request, Response>`。不能直接复用并克隆现有 `CrossbeamThreadPipe` 的双向 endpoint：克隆 Receiver 后多个调用方会竞争同一响应，可能取走别人的结果。

对外形状：

```rust
pub struct CommandIo { /* shared bounded request sender */ }
pub struct CommandCall { /* private one-shot receiver */ }

impl CommandIo {
    pub fn submit(&self, command: serde_json::Value)
        -> Result<CommandCall, SubmitError>;
}

impl CommandCall {
    pub fn try_result(&self) -> Option<serde_json::Value>;
    pub fn wait(self) -> serde_json::Value;
}
```

约束：

- request queue 容量 256。
- 每个请求携带自己的容量 1 one-shot response channel。
- 每个 Bevy frame 最多处理 64 条命令。
- 单条输入序列化后最大 64 KiB。
- 队列满时不阻塞；`roundo_cli` 将提交错误转成 `command_queue_full` JSON。
- 所有 handler 在 Bevy Update/main-world 上执行；Wry callback 与 stdin 线程不得直接访问 Bevy World。
- `app.quit` 先发送成功结果，再写 `AppExit`。

Web UI 对同一种 polling command 同时最多保留一个 pending 请求，避免在卡顿时累积。

## 11. Client Command 目录

### 11.1 通用/元命令

- `command.help`
- `command.schema`
- `dev`

### 11.2 Client

- `app.quit`
- `diagnostics.position`
- `ui.open`
- `client.interaction-mode.set`（Level 2；正常 UI 不应依赖它）
- `server.list`
- `server.refresh`
- `server.add`
- `server.edit`
- `server.delete`
- `server.connect`
- `server.retry`
- `server.disconnect`
- `server.status`
- `settings.show`
- `settings.set`
- `bindings.list`
- `bindings.bind`
- `bindings.unbind`
- `hud.show`

禁止重新加入 `game.enter/resume/pause/leave` 之类 Vanilla 流程命令。UI 通过原子能力组合自己的流程。

### 11.3 Server

现有 server CLI 也迁移到同一框架，至少包括：

- players list
- player position
- teleport
- app quit
- 通用 help/dev/schema

Client 与 Server 各有独立 Command Registry，共用框架和 Unix Adapter。终端统一打印 JSON，具体 compact/pretty 由 printer 决定。

## 12. 缓存和异步行为

命令返回不等待外部网络：

- `server.refresh`：发起已有 probe 逻辑，返回 `accepted` 与 refresh revision。
- `server.connect`：成功启动连接后返回缓存的 `connecting` 状态。
- `server.retry`：返回重新发起后的状态。
- `server.disconnect`：完成本地关闭后返回 `disconnected`。
- `server.status`：读取现有连接缓存。
- `server.list`：读取 Server Entry 与 probe 缓存。

菜单/连接 UI 每 250 ms poll；HUD 每 50 ms poll。缓存生产发生在现有网络/targeting/diagnostic 系统中，读取允许看到上一 frame 的稳定值。

展示模型必须包含 UI 所需约束，避免 Web UI 复制客户端逻辑：

- `settings.show`：key、当前值、min/max/step/default。
- `bindings.list`：支持的 key/action、显示名、当前关系。
- `server.list`：数组下标、名称、QUIC/HTTPS 地址、probe 状态与消息。
- `server.status`：disconnected/connecting/connected/reconnecting/error 及当前 Server Entry 快照。
- `hud.show`：FPS current/average/min/max、绝对坐标、chunk-relative 坐标、voxel target 或 null。

Server Entry 不具备唯一 ID；名称和地址均不要求唯一。命令按执行瞬间的当前数组下标操作，旧下标没有稳定性保证。

## 13. 配置迁移

将以下纯配置和持久化能力迁入 `roundo_user_config`：

- `ClientConfig`
- `ServerConfig`
- `ServerEntry`
- load/save 与旧配置兼容

顶层新增：

```toml
dev_mode = false
```

Client 与 Server 都有该字段，Serde default 为 false。本期不产生任何行为，不影响 `dev`、日志或功能开关。

Bevy `KeyCode` 与配置 enum 的转换可留在 `roundo_cli` client feature 或客户端应用层，不要把 Bevy 类型污染到纯配置数据。

## 14. Mod manifest 与优先级

Manifest 增加：

```toml
[general]
mod_name = "vanilla_ui"
author = "vanilla"
dependencies = ["base"]
load_priority = 0
```

### 14.1 名称

Mod ID 在运行时由 `<author>.<mod-name>` 组成。segment 必须匹配：

```text
[a-z0-9][a-z0-9_-]*
```

禁止大写和 segment 内点号。拒绝重复 Mod ID。

### 14.2 优先级

- 类型为 `i32`，默认 0，允许负数。
- 数值越大，加载/覆盖优先级越高。
- 相同优先级按 canonical Mod ID 升序加载；后加载者覆盖，因此字典序较大的 Mod ID 获胜。
- 依赖只决定合法拓扑/可引用集合，不隐式提升优先级。
- 同级冲突结果虽然确定，但 Mod 作者不应依赖字母顺序表达兼容关系。

### 14.3 验证

至少实现：

- manifest 扫描与解析
- 缺失依赖
- 循环依赖
- 依赖闭包
- 重复 Mod ID
- 通用 `ResourceCandidate` 优先级解析
- 确定性同级覆盖

任何已发现 Mod 的 manifest、依赖或 UI Registry 无效都使客户端启动失败；没有 UI Registry 是正常情况。

运行根目录固定为：

```text
std::env::current_dir()?.join("mods")
```

不使用 executable directory、环境变量或 debug/release fallback。

## 15. UI Registry

位置：

```text
mods/<mod>/assets/webui/registry.toml
```

示例：

```toml
[general]
initial = "main-menu"

[[ui]]
name = "main-menu"
project = "main-menu"
entry = "index.html"
interaction_mode = "web-ui"
world_visibility = "hidden"
prefetch = ["settings", "server-selection"]

[[ui]]
name = "hud"
project = "hud"
entry = "index.html"
interaction_mode = "in-game"
world_visibility = "visible"

[slots]
"roundo.main-menu" = "main-menu"
"roundo.hud" = "hud"
```

### 15.1 局部名与全名

Registry 中 `[[ui]].name` 只是单 segment 局部名。Loader 根据 manifest 生成：

```text
vanilla.vanilla_ui.main-menu
```

这从结构上阻止一个 Mod 定义另一个 Mod 名下的资源。拒绝同一 Mod 内重复局部 UI name。

### 15.2 引用

`general.initial` 和 slot value 可以引用：

- 当前 Mod 的局部 UI name；或
- dependency 闭包中资源的完整 UI Resource Name。

不能引用未声明依赖的已安装 Mod。

每个 Mod 最多提名一个 initial。所有 initial 提名竞争同一个全局 Initial UI Slot，使用 Mod Load Priority/同级 Mod ID 顺序决胜。该 slot 保存可追踪 UI Resource Name。

普通 `[slots]` key 是全局互斥位置。高优先级 Mod 可以让同一个 slot 指向自己的 UI Resource，但不会改变或伪造原资源身份。

`[[ui]].prefetch` 声明当前 Definition 可见时可能打开的相邻 Definition。它使用与 slot value 相同的局部名/依赖闭包引用规则。该声明只允许平台 Adapter 准备隐藏物理视图，不会打开 UI、修改 Lifecycle Tree 或消耗 `max_instances`。

### 15.3 Project 路径

- `project` 必须是 Mod 的 `assets/webui` 下相对目录。
- `entry` 必须位于该 project root 内。
- 解析 canonical path 后必须仍位于 project root。
- 禁止绝对路径、`..` 越界、symlink escape 和目录作为响应。

## 16. URL 与资源访问

只允许访问已注册 UI Resource 及其 project root 内 UI Asset。

支持：

```text
roundo-ui://slot/roundo.pause-menu/
roundo-ui://resource/vanilla.vanilla_ui.pause-menu/
```

- slot URL 先解析有效 UI Registry Slot。
- resource URL 直接解析 UI Resource Name。
- 相对 URL 留在当前 project。
- 导航到 slot 始终允许。
- 页面直接通过 resource URL 引用其他 Mod 时，当前资源所属 Mod 必须声明对应 dependency。
- 通过来源无关的 `ui.open { resource }` Client Command 打开具体资源不按调用方身份限制；这是完全信任且命令来源无关的既定约束。

禁止：

- `file://`
- `http://`
- `https://`
- 未注册 project 外的 Mod 文件
- 摄像头、麦克风、地理位置等 Web 权限

Custom protocol 应返回正确 MIME type，并对 HTML/JS/CSS/font/image 做一致的错误响应。首期不提供系统浏览器打开功能。

## 17. Wry/Bevy 集成

### 17.1 所有权

`roundo_webui` 提供深模块 `RoundoWebUiPlugin`：

- 等待 Bevy primary window 创建。
- 通过 `bevy_winit` 获取 Bevy 提供的原生窗口。
- 为每个已提交 UI Instance 创建或认领一个覆盖其 bounds 的 Wry child WebView。
- 已提交、加载中及已准备的 WebView 均由主线程 NonSend resource 保存。
- 窗口 resize 时更新 WebView bounds。
- 所有 Wry/WebView2/Windows 细节不暴露给 `roundo_client`。

### 17.2 每实例物理视图与图预取

每个已提交 UI Instance 独占一个 child WebView，使并发展示、独立 document state 和窗口化布局互不覆盖。生命周期销毁仍会销毁该实例拥有的物理视图。

平台 Adapter 根据当前可见 Definition 的 `prefetch` 邻接边，串行创建隐藏的 Prepared UI Candidate。候选必须完成 `about:blank`、目标文档导航及 bridge handshake，但 IPC command gate 在认领前保持关闭；Windows WebView2 在准备完成后进入 suspend，认领时 resume，避免隐藏页面持续运行 timer。候选没有 opener、生命周期 parent 或命令权限，不进入 Lifecycle Tree，也不消耗 `max_instances`。宿主当前最多保留 8 个候选，单个候选在认领前最多缓存 64 条启动命令。

真实 `ui.open` 命中相同 Definition/path 时，为候选绑定真实 source/parent 并沿普通 Pending UI Open commit 路径提交；未命中则保持冷加载行为。候选只可认领一次，不能作为两个实例共享的 document。候选在认领前没有 Root 身份，因此可跨 Root replacement 保留并在认领时绑定新 Root；Definition unload 或候选身份失效时通过安全 retirement 路径淘汰。

### 17.3 打开 UI 的原子语义

`open_ui(slot|resource, path)` 在改变当前状态前必须：

1. 解析 slot/resource。
2. 验证 dependency（适用时）。
3. 验证 project/entry/path。
4. 构造合法 URL。

全部成功并且物理视图加载、bridge handshake 完成后再原子应用：

1. 提交 Pending UI Open（命中 Prepared UI Candidate 时认领其物理视图）。
2. 应用资源声明的 `ClientInteractionMode`。
3. 应用 `UI World Visibility`。

运行中失败时保留现有页面、模式和 world visibility，并返回 typed JSON 错误。Initial UI 缺失或无效是致命启动错误。

### 17.4 Windows 输入模式

窗口由 Bevy 提供。Wry child HWND 通过 `wry::WebViewExtWindows` 获取。

`WebUi`：

- 移除 `WS_EX_TRANSPARENT | WS_EX_NOACTIVATE`。
- 聚焦 WebView。
- 鼠标 `visible = true`、`grab = None`。
- 禁用 `ClientPlayerController` 输入。

`InGame`：

- 给 child HWND 添加 `WS_EX_TRANSPARENT | WS_EX_NOACTIVATE`。
- 刷新 window style。
- 调用 `focus_parent()`。
- 鼠标 `visible = false`、`grab = Locked`。
- 启用游戏输入。
- WebView 保持可见并渲染 HUD。

窗口重新获得焦点时必须恢复当前 mode 对应的焦点和鼠标状态。

### 17.5 3D world visibility

由 UI Resource 独立声明：

| Resource | Interaction | World |
|---|---|---|
| Main/Servers/Connecting/Settings | WebUi | Hidden |
| HUD | InGame | Visible |
| Pause | WebUi | Visible |

3D camera active 条件不能再依赖旧 `ClientUiPhase`，而应读取当前 UI Resource 的 world visibility，并确保 camera 已绑定到 player controller。

### 17.6 失败

WebView2 Runtime 缺失、Wry child 创建失败或 Initial UI 无效时：

- 记录完整错误和可操作安装提示。
- 安全退出客户端。
- 不进入无 UI 状态。
- 不启用旧 Bevy UI fallback。

## 18. Web UI Command Bridge

注入 JS 形状：

```js
const result = await window.roundo.execute({
  version: 1,
  command: "server.connect",
  arguments: { index: 0 }
});
```

运输 envelope 可以包含 Adapter 私有 `request_id`：

```json
{
  "request_id": 17,
  "command": {
    "version": 1,
    "command": "server.connect",
    "arguments": { "index": 0 }
  }
}
```

`request_id` 不进入 Command Dispatcher：

1. JS Adapter 生成 ID 并记录 Promise。
2. Wry IPC callback 解析运输 envelope，向 `CommandIo` 提交其中 command `Value`。
3. `roundo_webui` 在 Bevy 主线程轮询 pending `CommandCall`。
4. 结果就绪后调用 `evaluate_script`，按 ID resolve/reject Promise。
5. 命令 JSON 结果原样交给页面；页面决定如何呈现。

普通浏览器独立开发时可以注入 mock `window.roundo.execute`。生产模式不要求 UI 独立 OS 进程。

## 19. 无客户端 UI 流程

客户端不得重新建立 S0～S5 phase enum。只有两类宿主入口：

- 启动时解析并打开 Initial UI Slot。
- 输入 Adapter/Client Command 调用统一 `open_ui` seam。

其余流程均由 UI Project 发命令和导航完成。例如连接页面 poll `server.status`，连接成功后自行执行 `ui.open` 打开 HUD；客户端不编码“连接成功就去 HUD”。HUD 发现断线后也由 UI 选择连接页或其他页面。

本期唯一硬编码输入 Adapter：

- 仅在 `InGame` 模式下由 Bevy 捕获 Escape。
- 调用 `open_ui(slot = "roundo.pause-menu")`。
- `WebUi` 模式下 Escape 由页面处理。

未来数据驱动输入模块应替换该检测，但复用同一个 `open_ui` seam。

## 20. Vanilla UI 资源布局

```text
mods/vanilla_ui/
├── manifest.toml
└── assets/webui/
    ├── registry.toml
    ├── shared/
    │   ├── index.html
    │   ├── roundo.css
    │   ├── bridge.js
    │   └── fonts/
    │       ├── NotoSans-Regular.ttf
    │       ├── NotoSans-SemiBold.ttf
    │       └── OFL-NotoSans.txt
    ├── main-menu/
    ├── server-selection/
    ├── connecting/
    ├── hud/
    ├── settings/
    └── pause-menu/
```

`shared` 本身是已注册 UI Resource；其中 CSS、JS、字体和许可证相关文件是 UI Asset，不单独注册。UI 使用的字体及对应许可证位于 Vanilla UI Mod。Project 使用原生 HTML/CSS/ES modules，不引入 npm/Node 构建步骤。

Vanilla 资源及 slot：

- `vanilla.vanilla_ui.main-menu` → `roundo.main-menu`
- `vanilla.vanilla_ui.server-selection` → `roundo.server-selection`
- `vanilla.vanilla_ui.connecting` → `roundo.connecting`
- `vanilla.vanilla_ui.hud` → `roundo.hud`
- `vanilla.vanilla_ui.settings` → `roundo.settings`
- `vanilla.vanilla_ui.pause-menu` → `roundo.pause-menu`
- `vanilla.vanilla_ui.shared`（通常直接 resource 引用）

Vanilla Registry 提名 main menu 为 initial。

## 21. 行为对等清单

### Main Menu

- 2.4 秒 `ROUNDO` intro。
- 淡入、缩放、淡出。
- 键盘或鼠标可跳过。
- Start、Settings、Quit。

### Server Selection

- 显示数组下标、名称和探测状态。
- 可选择、滚动和刷新。
- Add/Edit 表单包含名称、QUIC 地址、HTTPS 地址。
- Delete、配置持久化和保存错误显示。
- 选择状态、scroll、表单状态由 JS 拥有。

### Connecting

- 显示当前 Server Entry。
- 轮询缓存连接状态。
- 支持 retry/disconnect。
- 现有自动重连行为继续由网络层维护。

### HUD

- 透明背景。
- 居中准星。
- FPS current/average/min/max。
- 绝对坐标与 chunk-relative 坐标。
- WAILA：material/voxel、voxel position、relative position、chunk、Local Coordinate。
- 50 ms polling；同类请求不重叠。

### Settings

- Controls、Camera、Key Bindings tabs。
- Mouse、Movement、Targeting 折叠组。
- 鼠标灵敏度、camera speed、voxel raycast distance。
- slider、加减按钮、数值输入。
- 范围、step、default 来自 `settings.show`，不是 JS 常量。
- 完整 movement action/key binding 编辑。
- 页面 UI 状态由 JS/sessionStorage 拥有；实际设置由命令持久化。

### Pause

- 世界继续可见。
- 显示当前服务器。
- Continue、Settings、Leave 由 UI 组合 `ui.open`、`server.disconnect` 等原子命令。

## 22. 旧代码迁移

最终状态：

- 删除 `roundo_client/src/ui` 的旧 Bevy UI 模块和 `ClientUiState`。
- 删除 UI camera、widgets、Bevy text input、UI font loading、旧 presentation/action/lifecycle systems。
- 保留并改造 3D camera、targeting、diagnostics、cursor/focus 与 player input 控制。
- 将 `ClientNetworkManager`、`ActiveClientConnection`、`ServerProbeManager` 迁入 `roundo_cli` client feature。
- 将服务器 CRUD、连接、设置、binding、help/dev/schema handler 放入 `roundo_cli`。
- 客户端 targeting/diagnostics 系统更新 `roundo_cli` 可读取的 HUD cache。
- `roundo_client` 安装并连接 CLI/WebUI plugins，不维护 UI phase。

迁移完成后不保留 runtime feature flag 运行旧 UI。

## 23. 实施顺序

1. 在 `roundo_toolbox` 实现并测试 bounded request/response pipe。
2. 新建 `roundo_proc_macros`，实现 `UnixCommand` derive 与 compile-fail tests。
3. 在 `roundo_cli` 建立 typed JSON registry、schema、dispatcher、dev/help 和 Unix Adapter。
4. 将 Client/Server 现有终端命令迁移到新框架。
5. 将完整配置结构/持久化迁入 `roundo_user_config`，加入 `dev_mode`。
6. 将客户端连接管理、probe 与设置 handlers 迁入 `roundo_cli`。
7. 在 `roundo_mod_loader` 实现 manifest、依赖与 candidate resolver。
8. 在 `roundo_webui` 实现 UI registry importer、slot resolver、路径验证和纯逻辑测试。
9. 实现 Windows Wry child WebView、custom protocol、IPC Promise bridge、输入穿透和 resize/focus。
10. 创建 Vanilla UI Resources 并迁移字体/许可证。
11. 切换 `roundo_client` composition，接入 HUD cache 和 Escape Adapter。
12. 完成行为对等后删除旧 `roundo_client/src/ui`。
13. 运行 crate/workspace 测试并执行 Windows smoke test。

阶段可以内部逐步完成，但最终提交不能留下两套 UI 运行路径。

## 24. 自动化验证

至少覆盖：

1. 多来源 request/response 不串响应。
2. queue full、输入过大和 one-shot 行为。
3. command envelope、版本、未知命令、未知字段和参数错误。
4. schema 与真实 Input/Output 类型一致。
5. `UnixCommand` positional/options/default/usage。
6. proc macro compile-fail。
7. Mod manifest、缺失依赖、循环、依赖闭包、重复 Mod ID。
8. signed priority 与同级字典序覆盖。
9. UI local/full name、slot、initial 和 dependency reference。
10. path traversal、absolute path、symlink escape 和未注册资源访问。
11. 旧 Client/Server config 缺少 `dev_mode` 时为 false。
12. Server CRUD、probe cache、connection cache、settings 与 bindings handlers。
13. `ui.open` 校验失败时不改变现有 URL/mode/world visibility。
14. 纯逻辑测试不要求真实桌面 WebView。

Windows 手动 smoke test：

- 从 `current_dir()/mods` 载入 Vanilla UI。
- Initial UI 正确打开。
- WebView 覆盖 Bevy window 并响应 resize/DPI。
- 菜单可输入，InGame 可锁鼠标并控制 camera。
- HUD 可见且点击穿透。
- Escape 打开 pause，焦点与鼠标恢复。
- 跨 slot/resource 导航与 dependency 检查。
- 服务器 CRUD/probe/connect/reconnect/disconnect。
- settings/bindings 持久化。
- WebView2 缺失路径给出明确错误并退出。

## 25. 完成定义

只有同时满足以下条件才可宣称完成：

- Client/Server commands 已统一到 typed JSON command framework。
- Unix 输入只是 opt-in projection。
- Client/Server `dev_mode` 已加入且默认 false。
- Mod loader 能确定性解析 UI resources/slots/initial。
- Wry overlay 在 Windows 上实际运行。
- 六类 Vanilla UI 与 shared 资源已由 `mods/vanilla_ui` 提供。
- 现有 UI 行为清单已对等。
- InGame HUD 可见且不阻断 Bevy 输入。
- 旧 Bevy UI 运行路径已删除。
- 配置和 UI Resource 路径兼容性测试通过。
- 核心自动化测试及 Windows smoke test通过。
