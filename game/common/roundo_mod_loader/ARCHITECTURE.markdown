# Mod Resource Loading Architecture

本文描述 `roundo_mod_loader` 的目标技术架构。领域术语以仓库根目录 [`CONTEXT.md`](../../../CONTEXT.md) 为准；本文负责加载机制、验证阶段、adapter seam 与迁移约束。

当前实现只完成了 Mod 发现、manifest/依赖校验、依赖闭包和候选裁决。Web UI importer 仍自行组织资源加载；本文其余内容是后续重构必须收敛到的目标。

## 1. 设计目标

`roundo_mod_loader` 应成为一个 deep module：调用方只选择需要的根 Resource Type，module 隐藏 Mod 发现、类型依赖推导、并行加载、引用解析、错误聚合和原子发布。

核心设计哲学：

- **Mod 无加载顺序**：不能用目录顺序、manifest 顺序、依赖拓扑或优先级解释“哪个 Mod 先加载”。
- **Resource Type 有阶段顺序**：阶段只由编译期跨类型引用关系自动推导。
- **同类型批量处理**：一个阶段同时处理所有 Mod 对该 Resource Type 的贡献；同类型 Resource Declaration 不互相依赖。
- **一个事实来源**：类型引用关系同时定义合法跨类型引用和加载阶段，不另设手写顺序。
- **按进程选择**：共享 lib 定义全部 Resource Type；客户端和服务端只选择根类型，实际集合由依赖闭包推导。
- **阶段原子性**：一个 Resource Type 的全部资源成功后才一起发布；任何错误终止整个初始化。
- **初始化后不可变**：本架构不预留运行时增删 Mod、registry 热重载或目录回滚语义。
- **registry 才是资源入口**：裸文件只是 payload；未经 registry 声明，不能成为 Mod Resource。

## 2. 职责与 seam

### 2.1 共享加载 module

共享 module 拥有：

- canonical Mod ID、manifest 解析与重复检测；
- 显式、无环的 Mod Dependency graph；
- 完整、无序的编译期 Resource Type Catalog；
- 客户端/服务端 Resource Type Selection 的依赖闭包；
- Resource Type Dependency graph 的编译期验证与阶段推导；
- 每个阶段的 registry 收集、并行加载、确定性错误聚合与原子发布；
- 强类型 Mod Resource Name 与 dependency closure 可见性；
- 初始化完成后的不可变已加载资源目录。

它不拥有：

- Wry/WebView 生命周期；
- Bevy ECS、`AssetServer`、Handle 或运行时资产构造；
- 某个 Resource Type 的业务字段和类型特有后处理；
- UI Registry Slot、UI Prefetch 等 Web UI 规则；
- 网络同步、存档或热重载。

### 2.2 Resource Type adapter

每个 Resource Type adapter 位于共享加载 seam，负责该类型独有的：

- registry payload schema；
- 编译期允许引用的其他 Resource Type；
- declaration 内容验证；
- payload 文件加载与转换；
- 类型特有的批量后处理；
- 生成该类型的不可变已加载值。

Resource Type adapter 不扫描 Mod、不解释 Mod Dependency、不决定阶段顺序，也不单独发布部分结果。

### 2.3 平台 adapter

客户端或服务端的平台 adapter 消费已加载资源数据，并执行 Wry、Bevy 或其他运行时行为。Bevy `AssetServer` 可以读取经过验证的 payload，但不能成为平行的 Mod 发现或身份系统。

## 3. 编译期 Resource Type 模型

### 3.1 Resource Type Catalog

所有 Resource Type 在一个共享 lib 中显式、无序地汇总。禁止链接器式隐式发现。显式 Catalog 提供 locality：完整类型集合可以被审阅、测试和静态验证。

Catalog 不记录加载顺序。顺序由类型关系推导。

### 3.2 类型引用关系

Resource Type 在编译期规定它可以引用哪些其他 Resource Type。该关系就是 Resource Type Dependency，不能由 registry 增加、删除或改变。

不存在：

- registry 声明的类型依赖；
- 仅用于排序的类型依赖；
- 手写阶段序号；
- 同类型加载依赖。

类型图必须在编译期拒绝：

- 重复 Resource Type identity；
- 指向 Catalog 外类型的关系；
- 指向自身的关系；
- 任意依赖环；
- 无法生成确定性阶段的定义。

### 3.3 进程选择

客户端和服务端依赖同一个 Catalog，但各自只声明直接消费的无序根类型。有效加载集合是根类型在 Resource Type Dependency graph 上的闭包。

进程只发现、解析和验证有效集合中的 registry。例如服务端不选择 Web UI，就不会读取或发现损坏的 Web UI registry。完整 Mod 包校验应由选择全部所需根类型的独立工具完成。

## 4. Mod 模型

### 4.1 发现与身份

只有 `mods/<directory>/manifest.toml` 存在且有效的目录才是 Mod。canonical Mod ID 由 manifest 的 `author` 与 `mod_name` 组成；目录名不提供资源身份。

没有 manifest 的目录即使位于 Bevy asset root 下，也不能贡献 Mod Resource。

### 4.2 Mod Dependency

Mod Dependency 在 manifest 中显式声明，必须存在且无环。它表示：

- 安装要求；
- 对依赖闭包中资源的引用可见性。

它不表示：

- Mod 加载顺序；
- Resource Type 阶段顺序；
- override priority；
- 从具体资源引用自动推导出的隐式依赖。

### 4.3 Override Priority

manifest 使用 `override_priority`。它不参与加载。具体 Resource Type 可按自身规则使用这个值裁决候选，例如 Web UI adapter 裁决 UI Registry Slot。旧 `load_priority` 只保留为反序列化迁移别名。

共享加载 module 不建立“排他资源”或“资源被占用”的通用模型。

## 5. Registry 约定

### 5.1 固定位置

每个 Mod、每种 Resource Type 最多拥有一个 registry：

```text
mods/<mod>/assets/<resource-type>/registry.toml
```

registry 缺失表示该 Mod 不贡献此类型资源，不是错误。禁止递归搜索 `*_registry.toml` 或允许每个 Mod 自定义 registry 位置。

### 5.2 共同外壳

所有 Resource Registry 使用统一声明风格：

```toml
[[resource]]
name = "local-name"
# 其余字段由 Resource Type 定义
```

固定内容只有：

- `[[resource]]` 条目外壳；
- 当前 Mod 内唯一的本地 `name`。

Resource Type 由 registry 路径和编译期 adapter 确定，条目不重复填写类型。类型特有顶层 section 可以存在，但不能改变资源身份、Mod 可见性或类型依赖。

### 5.3 Payload

文本、二进制或目录 payload 可以位于该 Resource Type 的资产目录中，由 declaration 引用。路径必须经过类型 adapter 验证，禁止绝对路径、`..` 越界和 symlink escape。

文件存在本身不产生资源身份。未声明 payload 可以随 Mod 分发，但调用方不能把其裸路径当作 Mod Resource。

## 6. 强类型资源身份与引用

资源真实身份是：

```text
(Resource Type, canonical Mod ID, local name)
```

因此同一个 Mod 可以在不同 Resource Type 中复用相同 local name。registry 字段已由编译期 schema 确定目标 Resource Type，不重复书写类型。

引用规则：

- 当前 Mod 资源可以使用 local name；
- 依赖 Mod 资源必须使用包含 canonical Mod ID 的完整名称；
- 完整名称的 owner 必须位于当前 Mod 的 dependency closure；
- 禁止在依赖集合中按 local name 搜索；
- 禁止依赖声明顺序参与解析；
- 禁止资源引用隐式创建 Mod Dependency 或 Resource Type Dependency。

同类型 Resource Declaration 不建立加载依赖。某种类型若拥有 slot、索引或其他类型特有的间接选择结构，由该 adapter 在本阶段全量收集后处理，不进入 Resource Type Dependency graph。

## 7. 加载算法

一次进程初始化遵循以下逻辑：

1. 发现所有具有 manifest 的 Mod。
2. 验证 canonical Mod ID、manifest、显式依赖、缺失依赖和依赖环。
3. 从进程根 Resource Type 推导有效类型闭包。
4. 从编译期类型关系推导确定性 Resource Type Loading Phase。
5. 按阶段推进；同阶段互不依赖的 Resource Type 可以并行。
6. 对每个 Resource Type：
   1. 按固定路径收集所有 Mod 的 registry；
   2. 解析全部 `[[resource]]` declaration；
   3. 并行加载和验证 declaration/payload；
   4. 解析对已完成类型的强类型引用；
   5. 执行该类型的全量后处理；
   6. 聚合并确定性排序全部错误；
   7. 无错误时原子发布该类型目录。
7. 所有有效类型发布后，将完整目录交给平台 adapter。
8. 程序初始化完成后，Mod 集合和资源目录不可变。

“并行”只属于 implementation。interface 不暴露任务完成顺序，任何输入枚举顺序都不能改变发布结果、候选裁决或错误顺序。

## 8. 错误模型

一个 Resource Type 阶段不 fail-fast。所有能够安全执行的 declaration 都应完成验证，错误按以下稳定键聚合：

```text
canonical Mod ID → local resource name → error kind
```

典型错误包括：

- registry TOML/schema 无效；
- 同一 registry 中 local name 重复；
- payload 缺失、类型错误或路径不安全；
- 完整资源名格式无效；
- 未声明 Mod Dependency；
- 已声明但不存在的目标资源；
- 类型特有的全量后处理失败。

任何错误都阻止该 Resource Type 发布，并阻止依赖阶段启动。架构假设阶段能够完整枚举该类型所有 declaration 和同层级错误，不引入“错误集合可能不完整”的状态。

## 9. 测试面

共享 module 的 interface 是主要测试面。至少覆盖：

- Mod 输入枚举随机化不改变结果；
- 显式 Mod Dependency、传递闭包、缺失和成环；
- 根 Resource Type Selection 自动扩展闭包；
- 类型阶段从编译期关系确定性推导；
- 类型图的重复、自引用、未知类型和成环在编译期失败；
- 同阶段 Resource Type 可并行且互不观察部分结果；
- 单类型跨 Mod 原子发布；
- 多错误聚合具有稳定顺序；
- 本地名与完整名解析；
- dependency closure 外引用失败；
- 不同 Resource Type 可复用同一 local name；
- registry 缺失与 payload 未注册的语义；
- 进程未选择的 Resource Type registry 不被读取。

每个 Resource Type adapter 另行测试自己的 schema、payload 转换和全量后处理。平台 adapter 只测试运行时接线，不重复测试通用 Mod 规则。

## 10. 当前迁移差距

当前代码与目标之间至少存在以下差距：

- `LoadedMods` 已实现 manifest、Mod ID、依赖闭包和依赖环校验，但没有 Resource Type Catalog 或阶段推导。
- `roundo_webui::UiRegistry::load` 自行扫描 registry、解析引用和发布结果，尚未作为 Resource Type adapter 接入共享阶段。
- Web UI registry 已使用共同 `[[resource]]` 外壳；旧 `[[ui]]` 只作为迁移别名。
- Web UI `prefetch` 已引用 UI Registry Slot，并在 slot 最终裁决后解析。
- manifest 已使用 `override_priority`，旧 `load_priority` 只作为迁移别名。
- `mod_assets` 将整个 `mods/` 暴露给 Bevy，尚未阻止裸路径绕过 registry。
- 示例 USD、Attribute、Property、Form、Material、Recipe 与 Action 文件尚无接入共享加载模型的 adapter。

迁移时应优先建立第二个真实 Resource Type adapter，再抽取 Web UI 与第二个 adapter 共同需要的 seam，避免用单一 adapter 推导 hypothetical interface。
