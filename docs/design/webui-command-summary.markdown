# Web UI 与 Client Command 重构摘要

> **状态**：设计已确认，尚未实现。

## 为什么重构

现有 Bevy UI 同时承担视觉、连接、配置、设置和状态机职责，导致客户端能力与 UI 强耦合，无法由 Mod 替换。目标是让客户端只提供能力，让 UI 只负责交互、视觉和自己的页面状态。

## 核心设计

### 1. 客户端能力统一为 typed JSON commands

- 命令输入和返回值都是带版本的 typed JSON。
- 终端、Web UI、Bot 等都是 Adapter；调用方身份不改变执行语义。
- Rust command definitions 是输入/输出 JSON Schema 的唯一真值。
- `roundo_cli` 拥有命令目录、handler、help、schema 和执行逻辑。
- 网络命令立即返回；读取命令只读缓存，不等待网络往返。

**原因**：避免 UI 直接调用客户端内部函数，也避免终端和 Web UI 各维护一套语义。

### 2. Unix 命令只是可选投影

命令若需要终端形式，就实现 `UnixCommand`。`roundo_proc_macros` 中的 derive 自动生成 Unix 参数到 typed JSON 的转换；没有实现 trait 的命令就没有 Unix 版本。

**取舍**：保留 Unix 使用习惯，但 JSON 才是客户端接口，防止文本解析成为系统真值。

### 3. 所有请求必须有独立 JSON 响应

使用 bounded request/response pipe；每个请求携带自己的 one-shot response channel，不共享响应 Receiver。

**原因**：多个来源并发时不能串响应；bounded queue 也防止 HUD polling 或外部调用无限积压。

### 4. UI 是 Mod 注册的 Web UI Resource

- 资源全名运行时生成为 `<author>.<mod-name>.<resource-name>`。
- 资源身份与全局 UI Registry Slot 分离。
- 高优先级 Mod 通过让同一个 slot 指向自己的资源实现替换，而不是冒用其他 Mod 的资源名。
- Initial UI 也由一个全局互斥 slot 决定。

**原因**：同时满足可追踪来源与可替换性。

### 5. Mod 冲突必须确定性解析

- `load_priority: i32`，越大越优先。
- 同优先级按 Mod ID 升序加载，字典序较大者最终覆盖。
- 跨 Mod 引用必须声明 dependency。
- 非法 manifest、依赖、registry 或资源路径导致启动失败，不静默跳过。

**取舍**：同级覆盖不作为兼容承诺，但结果必须可复现，不能随机。

### 6. 一个 Wry WebView 覆盖 Bevy 窗口

- 窗口由 Bevy 创建，Wry 作为 child WebView 覆盖其 client area。
- 首期只支持 Windows。
- WebView2/Wry 或 Initial UI 失败时安全退出，不保留 Bevy UI fallback。
- 只允许访问已注册 UI Resource 及其 project 内资产，不允许 file/http/https 导航。

### 7. 输入模式与世界可见性分离

- `WebUi`：WebView 接管输入，鼠标释放。
- `InGame`：Bevy 接管输入，鼠标锁定；WebView 保持渲染 HUD，但点击穿透。
- UI Resource 另外声明 3D world 是否可见。

**原因**：暂停菜单需要显示世界但接管输入；主菜单需要接管输入但隐藏世界；HUD 需要显示世界且不能拦截输入。

### 8. 客户端不拥有 UI 流程

客户端启动时只打开 Initial UI。连接成功后去哪、设置页如何返回、断线显示什么，都由 Web UI 自己通过原子 commands 和超链接决定。

本期唯一宿主按键特例是 InGame 下的 Escape：它调用统一 `open_ui` seam 打开暂停 slot。未来由数据驱动输入模块替换。

## Vanilla UI 迁移

现有 UI 将拆为 Mod Resources：

- Main Menu
- Server Selection
- Connecting
- HUD
- Settings
- Pause Menu
- Shared CSS/JS/fonts

必须保留 intro、服务器 CRUD/probe、连接/重连状态、HUD/WAILA、设置、完整按键绑定和暂停行为。字体与许可证迁入 `mods/vanilla_ui`；字体是 Project 内资产，不单独注册。

迁移完成后删除旧 `roundo_client/src/ui`，只保留并改造 3D camera、targeting、diagnostics、焦点和游戏输入逻辑。

## 配置

`ClientConfig`、`ServerConfig`、`ServerEntry` 和持久化迁入 `roundo_user_config`。Client/Server 顶层都新增：

```toml
dev_mode = false
```

该字段本期无行为，也与运行时 `dev [LEVEL]` 无关。

## 重要约束

- Mods 固定从 `current_dir()/mods` 加载。
- 所有 Mod 完全可信；命令没有权限等级。
- `dev` 只过滤 help，不阻止执行。
- 服务器名称、地址不唯一；命令使用当前列表数组下标。
- 一个 WebView 中页面导航会卸载旧页面；需要保留的 UI 临时状态由 Project 使用 `sessionStorage` 保存。
- 不引入 npm/Node 构建步骤。
- 不实现完整未来输入模块，不维护两套 UI，不提供外部网页访问。

## 关键取舍

- 选择 JSON 而非 Unix 文本作为接口：获得可验证 schema、多 Adapter 一致性和结构化返回。
- 选择显式命令注册而非 linker inventory：依赖和测试更清晰。
- 选择 Resource Name/Slot 分离：避免“可替换”和“可追踪”互相冲突。
- 选择单一 WebView：减少原生视图、焦点和生命周期复杂度；代价是 Project 跨导航状态必须自行持久化。
- 选择 Windows-first：先保证 Wry child overlay、输入穿透和 Bevy window 集成可靠，再扩展平台。
- 选择彻底删除 Bevy UI fallback：避免长期维护两套行为和产生不一致。

详细实施规范见 `docs/design/webui-command-architecture.markdown`。
