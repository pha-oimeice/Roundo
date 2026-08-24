# UI 生命周期树重构摘要

## 目标

将当前“单个 WebView + 页面硬编码跳转”的 UI 流程重构为由宿主管理的生命周期树，使页面只知道自己要打开什么，而不知道自己的父节点、所在树或返回目标。

## 核心模型

- 客户端 UI 状态机只保留 `Disconnected` 与 `Connected` 两种状态。
- 每种状态拥有一个必然存在、不可见的 Root Anchor。
- `roundo.disconnected-root` 与 `roundo.connected-root` slot 都是可选的；缺失时不加载 Root UI。
- 每次 `ui.open` 创建新的 UI Instance。实例拥有独立 WebView、DOM/JS 状态及生命周期身份。
- 调用 `ui.open` 的 UI Instance 自动成为新实例的父节点；非 UI 来源默认挂到当前 Root Anchor。
- `ui.back` 销毁调用实例，不再硬编码打开某个页面。

## 销毁和独立性

注册项新增：

```toml
max_instances = 1
lifecycle_independent = false
presentation = "exclusive"
layout = "fullscreen"
```

- 被直接要求销毁的实例必然销毁，它自身的 `lifecycle_independent` 不保护自己。
- 父节点被销毁时，普通后代递归销毁。
- 遇到首个 `lifecycle_independent = true` 的后代时，该实例及其完整子树作为幸存边界，重挂到最近仍存活的祖先。
- Root Replacement 和应用退出无视独立性，销毁整棵树。
- 销毁先计算最终销毁集合、幸存边界、父关系、焦点和计数，再原子提交。

## 连接语义

- 游戏启动时一定处于 Disconnected Root。
- Connecting UI 仍位于 Disconnected 树。
- 只有权威 `Connected` 事件建立 Connected Root；只有权威 `Disconnected` 事件建立 Disconnected Root。
- UI 打开、Back、销毁都不能隐式连接或断开。
- 进入 settings 不会断线；从 settings Back 会关闭 settings 并恢复其实际父界面。
- Connected Root 始终开启游戏摄像头；页面只决定输入和世界是否从其视口透出。

## 并发呈现

- 生命周期树不是视觉栈。
- `exclusive` 隐藏父呈现组但不销毁或卸载父实例。
- `concurrent` 允许父子及兄弟实例同时显示。
- `fullscreen` 覆盖客户端区域；`windowed` 使用宿主管理的位置、尺寸、层级和焦点。
- 每个实例保留独立 WebView，因此物品栏可以同时打开多个物品信息窗口。
- 只有一个 Focused UI 接收 Web UI 输入；点击未被 Web UI 覆盖的区域可在 Connected 状态恢复游戏输入。

## 注册与命令

- 每个 UI Definition 必须声明正整数 `max_instances`；计数是进程级逻辑存活数，创建提交时增加，逻辑销毁时减少。
- 不设全局实例或 WebView 上限。
- 任意 UI 可以打开任意已注册 UI；不维护 opens 权限图。
- 页面请求体不包含实例 ID。宿主为所有 WebView 命令绑定不可伪造的来源上下文。
- 已销毁或卸载来源产生 `stale_ui_instance`；已被业务服务接受的命令不会因来源销毁而撤销。
- 跨 Definition 顶层导航和 iframe 加载不得绕过 `ui.open`。

## 故障处理

- open 先创建不可见 staged WebView，等待导航完成及桥握手，之后才提交实例。
- Root Replacement 会取消旧根下所有 Pending UI Open。
- 非 Root UI 加载失败时来源和树保持不变。
- Root UI 加载失败时显示宿主 Recovery Surface；它不属于注册表或生命周期树。
- WebView/HWND 清理失败不回滚逻辑销毁。
- 内部树不变量破坏时 fail fast；页面或 Mod 的非法请求返回结构化错误且树不变。

## 关键验收场景

```text
ConnectedRoot
└─ HUD
   └─ Inventory
      ├─ ItemInfo A (false)
      ├─ ItemInfo B (true)
      └─ ItemInfo C (false)
```

Inventory Back 后，Inventory/A/C 销毁；B 及其后代重挂到 HUD，保留 WebView 状态；连接和摄像头不受影响。随后 Root Replacement 必须无条件销毁包括 B 在内的整棵旧树。
