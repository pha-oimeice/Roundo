# Web UI Resource Type Adapter

本文描述 Web UI 如何接入共享 Mod Resource 加载架构。通用加载规则见 [`roundo_mod_loader/ARCHITECTURE.markdown`](../../game/common/roundo_mod_loader/ARCHITECTURE.markdown)，UI 生命周期术语见仓库根目录 [`CONTEXT.md`](../../CONTEXT.md)。

本文是目标技术文档。当前 `UiRegistry::load` 已实现 Mod 注册 UI、完整资源名、dependency closure、slot 裁决和路径验证，但尚未接入统一 Resource Type Loading Phase。

## 1. Adapter 职责

Web UI Resource Type adapter 只拥有 Web UI 特有规则：

- UI Definition declaration schema；
- project root 与 entry 文件验证；
- interaction、world visibility、presentation、layout、instance limit；
- UI Registry Slot 提名与裁决；
- UI Prefetch slot 校验；
- UI 类型阶段的全量后处理；
- 生成不可变 UI Registry 数据。

它不拥有：

- Mod 扫描或 manifest 解析；
- Mod Dependency graph；
- Resource Type 阶段排序；
- 通用 local/full name 解析；
- Wry WebView 创建与生命周期；
- UI Instance、Lifecycle Tree 或运行时导航。

Wry 仍是消费已加载 UI Definition 的客户端平台 adapter。

## 2. Registry

固定位置：

```text
mods/<mod>/assets/webui/registry.toml
```

目标 declaration 采用共同外壳：

```toml
[[resource]]
name = "settings"
project = "settings"
entry = "index.html"
interaction_mode = "web-ui"
world_visibility = "hidden"
max_instances = 1
prefetch = ["roundo.pause-menu"]
```

其中：

- `name` 是当前 Mod 内的 UI Definition local name；
- `project` 是 `assets/webui/` 下的项目目录；
- `entry` 是 project root 内的入口文件；
- `prefetch` 只填写 UI Registry Slot，不直接填写 UI Definition local/full name；
- 其他字段继续由 Web UI adapter 定义和验证。

`registry.toml` 缺失表示该 Mod 不提供 Web UI，不是错误。

## 3. UI Registry Slot

UI Registry Slot 是 Web UI Resource Type 自己的选择规则，不是通用资源“排他性”或“占用”模型。

一个 Mod 可以提名：

```toml
[slots]
"roundo.settings" = "settings"
"roundo.pause-menu" = "pause-menu"
```

slot value 使用通用资源引用规则：

- local name 引用当前 Mod 的 UI Definition；
- full name 引用显式 Mod Dependency closure 中的 UI Definition；
- 不搜索依赖顺序，不改变 UI Definition 身份。

多个 Mod 提名同一个 slot 时，Web UI adapter 使用 manifest 的 `override_priority`，同优先级再按 canonical Mod ID 确定性裁决。该优先值不影响 Mod 或 Resource Type 加载顺序。

UI Definition 可以：

- 不被任何 slot 选中；
- 被一个 slot 选中；
- 被多个 slot 选中。

因此 slot 不占用、不替换、不重命名，也不限制 UI Definition。

## 4. UI Prefetch

UI Prefetch 从一个 UI Definition 指向 UI Registry Slot：

```text
UI Definition → UI Registry Slot → selected UI Definition
```

禁止直接建立：

```text
UI Definition → UI Definition
```

理由：

- prefetch 应跟随 slot 的最终裁决；
- 高优先级 Mod 替换 slot 后，调用方无需修改；
- UI Definition 之间不形成同 Resource Type 加载依赖；
- slot 是 UI 类型阶段内部的全量选择结果。

prefetch 只准备 Prepared UI Candidate，不创建 UI Instance，不改变 Lifecycle Tree，不消耗 live-instance count，也不授予命令权限。

## 5. Web UI 类型阶段

Web UI Resource Type Loading Phase 必须按以下逻辑原子完成：

1. 收集所有 Mod 的 Web UI registry。
2. 解析全部 UI Definition declaration。
3. 验证 local name、project、entry、layout 与类型特有字段。
4. 生成强类型完整资源身份。
5. 收集全部 slot 提名。
6. 验证 slot value 的 Mod Dependency 可见性与目标存在性。
7. 使用 `override_priority` 和 canonical Mod ID 裁决每个 slot。
8. 校验每个 UI Prefetch 所引用的 slot。
9. 验证所有类型内部不变量。
10. 无错误时一次性发布 UI Definition、slot map 和 prefetch slot map。

任何错误都使整个 Web UI 类型阶段失败。运行时不能观察只有部分 Definition、部分 slot 或未校验 prefetch 的 UI Registry。

## 6. Payload 与协议读取

HTML、CSS、JavaScript、字体和其他 project 文件是 UI Definition 的 payload。Web UI adapter 在加载阶段验证 project root 和 entry；运行时 custom protocol 只能从已注册 UI Definition 的 validated project root 读取相对路径。

必须拒绝：

- 未注册目录的裸 `mods/...` 路径；
- absolute path；
- `..` 越界；
- symlink escape；
- 目录被当作文件；
- 跨 UI Definition project root 读取。

Wry custom protocol 是文件响应 adapter，不承担 Mod Resource 发现。

## 7. 与 UI 生命周期的 seam

加载完成的 UI Registry 是不可变输入。`UiLifecycleManager` 消费它来：

- 通过 slot 解析打开目标；
- 创建 Pending UI Open；
- 管理 UI Instance count；
- 创建 Prepared UI Candidate；
- 执行 Root Replacement 与 Lifecycle Destruction。

Resource Type adapter 不创建 UI Instance。Lifecycle Tree 也不反向修改 UI Registry。加载错误发生在运行时生命周期建立前；WebView 创建或页面加载失败则属于平台 adapter 和 Recovery Surface 的运行时错误。

## 8. 当前实现差距

当前 `roundo_client/roundo_webui/src/lib.rs` 需要迁移的内容：

- `RoundoWebUiPlugin::build` 当前自行调用 `LoadedMods::discover`；目标由共享加载 module 提供已加载 UI Registry。
- `UiRegistry::load` 混合通用 Mod 规则和 Web UI 类型规则；目标只保留后者。
- `RegistryFile` 使用 `ui: Vec<RegistryUi>` / `[[ui]]`；目标是 `resource` / `[[resource]]`。
- `resolve_reference` 中通用 Mod 可见性应迁入共享加载 module。
- `RegistryUi.prefetch` 当前解析为具体 UI Resource Name；目标保存并校验 UI Registry Slot。
- manifest 当前使用 `load_priority`；目标字段为 `override_priority`。
- 当前插件同时拥有 registry 加载与 Wry runtime；目标是 Resource Type adapter 与平台 adapter 两个 seam。

迁移不得改变 ADR-0002 已确定的原则：UI Definition 的可追踪身份与 UI Registry Slot 必须分离，高优先级 Mod 只能改变 slot 的选择结果，不能冒充另一 Mod 的 UI Definition。

## 9. 测试面

Web UI adapter 测试至少覆盖：

- 多 Mod UI Definition 批量注册和原子发布；
- 同一 Mod 重复 local name；
- local/full UI Definition 引用与 dependency closure；
- slot value 指向未知或不可见 UI Definition；
- `override_priority` 与 canonical Mod ID 的确定性 slot 裁决；
- 一个 UI Definition 被零个、一个或多个 slot 选中；
- prefetch 只接受已解析 slot；
- slot 替换会让 prefetch 跟随新胜者；
- project/entry/path traversal/symlink escape；
- 任意 Mod 枚举顺序产生相同 Registry 和错误顺序；
- 阶段失败时不暴露部分 Definition、slot 或 prefetch。

UI Lifecycle Tree 和 Wry adapter 的测试继续通过各自 interface 进行，不应重复 registry 加载规则。
