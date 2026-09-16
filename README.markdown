# Roundo

Roundo 是一个 Rust/Bevy workspace：客户端负责输入、UI、网络适配与表现，服务端负责连接、权威玩家/体素状态、物理与数据库接入。客户端和服务端通过 QUIC 通信；共享 wire DTO 位于 `roundo_contracts`。

> 仓库正在开发中。crate 或类型存在不代表已经接入运行路径；设计文档也不自动代表当前实现。

## 从哪里开始

| 目标 | 文档 |
| --- | --- |
| 查 API 的不变量、失效、所有权、失败与并发语义 | [`API_CONTRACTS.markdown`](API_CONTRACTS.markdown) |
| 查 Mod Resource 加载模型 | [`game/common/roundo_mod_loader/ARCHITECTURE.markdown`](game/common/roundo_mod_loader/ARCHITECTURE.markdown) |
| 查 Web UI Resource adapter | [`roundo_client/roundo_webui/RESOURCE_LOADING.markdown`](roundo_client/roundo_webui/RESOURCE_LOADING.markdown) |

各 crate 的 item 级事实以源码 rustdoc 为准。工作树中可能另有未纳入版本控制的研究笔记；它们不自动构成当前实现契约。

阅读文档时区分四类陈述：

- **已实现**：代码存在且接入当前运行路径。
- **局部实现**：数据结构或部分路径存在，但端到端能力不完整。
- **设计目标**：约束后续实现，不是当前能力声明。
- **未明确**：当前没有可依赖的契约；调用方必须避免自行假定。

## Workspace

主要边界如下：

- `roundo_client` / `roundo_server`：进程级组合根。
- `roundo_ecs_entry`：客户端和服务端 Bevy App 装配。
- `roundo_contracts`：版本化 wire identity、DTO、逻辑流和协议版本。
- `roundo_networking`：postcard frame、QUIC session、TLS、重连与有界发送准入。
- `roundo_local_coordinate`：权威体素、Chunk、SVO、观察、raycast 与 collider 派生。
- `roundo_marionette`：Mod Input Registry、玩家 controller command、移动、旋转与交互消息。
- `roundo_presence`：连接对应的 Player 生命周期、Scene 与 presence snapshot。
- `roundo_rendering`：客户端 Render Object 与 World Render Pipeline。
- `roundo_mod_loader` / `roundo_webui`：Mod Resource 基础模型及 Web UI adapter。
- `roundo_lifecycle`：UI ownership/lifetime tree 的纯数据模型。
- `roundo_toolbox`：进程内 pipe、bridge runtime、registry 等通用机制。

完整成员列表和直接依赖以根 [`Cargo.toml`](Cargo.toml) 为准。

## 构建

在 workspace 根目录执行：

```text
cargo build --workspace --profile dev --features dev
```

测试应以可重复的断言、日志或计数器验证；截图和目测不构成通过证据。
