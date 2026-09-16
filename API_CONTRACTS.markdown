# Roundo API 契约

本文是跨 crate 的语义入口。局部 API 的最终契约应与实现放在同一 Rust item 的 rustdoc 中；本文只收录调用方需要同时理解的跨边界规则，并指出尚未定义的部分。

## 文档完成标准

新增或修改公开 API 时，先记录其**合法状态和不变量**，再记录**什么会失效**。其余契约只在适用时写入；不要用“显然”“正常情况下”代替可检查的条件。

| 优先级 | 契约 | rustdoc 必须回答的问题 |
| --- | --- | --- |
| 极高 | Validity / invariants | 哪些输入和状态合法？类型本身是否保证合法，还是需要调用者先验证？ |
| 极高 | Invalidation | 哪些 mutation、commit、despawn、disconnect 或 rebuild 会使引用、ID、handle、iterator、plan、cache 失效？ |
| 极高 | Structural semantics | 插入/删除影响一个值还是整棵子树？是否移动数据、改变索引、折叠表示或延迟到 schedule？ |
| 极高 | Ownership / lifetime | 返回值是 owned snapshot、共享 snapshot、借用还是逻辑 handle？它能活到何时？`clone` 是复制值还是共享同一后端？ |
| 高 | Mutation | 立即生效、下一 schedule 生效、阶段提交、copy-on-write，还是只修改 derived cache？ |
| 高 | Failure | 每个 `Err` / `None` / panic 分支分别表示什么？失败后输入值是否归还？ |
| 高 | Ordering | 顺序是 insertion、sorted、stream-local、unspecified，还是只有 partial order？ |
| 高 | Concurrency | 是否 `Send`/`Sync`、可重入、阻塞、有界、会取消 future，或要求固定线程？ |
| 高 | Side effects | 是否执行 I/O、发消息、改 ECS/全局状态、创建线程、填充 cache 或记录日志？ |
| 高 | Atomicity / consistency | 失败前是否可能部分修改？何时对其他组件可见？多个对象是否一起提交？ |
| 高 | Identity | ID 的签发者、作用域、复用规则；`clone`、序列化和重连后是否仍表示同一对象？ |
| 高 | Units / coordinates | 单位、local/world、Chunk/voxel、轴顺序、边界和取整方式。 |
| 中到高 | Determinism | 相同输入是否得到相同值和顺序？若依赖 HashMap、线程完成顺序或多个 QUIC stream，应明确不可依赖。 |
| 按需 | Complexity | 会扫描、分配、clone 大 payload、阻塞或触发 O(n) rebuild 时写明。 |
| 必须 | Safety | 每个 `unsafe` block/函数旁写出由谁保证的前置条件，以及相关指针、COM/Win32 handle 和线程约束。 |

“不适用”不需要形成模板噪声；但公开 mutation 若没有说明 invalidation、失败原子性和可见时点，文档不算完成。行为不明确时写“未定义/不可依赖”，而不是从当前容器或调度偶然推导保证。

## 当前跨边界契约

### Identity 与合法状态

- `ConnectionId`、`PlayerId`、`LocalCoordinateId`、`RenderObjectId`、`UserId` 和 `SessionId` 属于不同 identity seam，不能按数值互换。
- `ConnectionId` 和 `PlayerId` 是运行期身份。断线重连会获得新身份；调用方不能把旧 ID 当作持久用户身份。
- `ChunkId` 由 `LocalCoordinateId + [i64; 3]` Chunk 坐标组成；`ChunkVersion` 再附加服务端签发的内容版本。客户端只能比较和缓存版本，不能自行推进权威版本。
- `AtomicVoxelId(0)` 保留为空空间；Mod 注册的 Atomic Voxel 必须使用非零 ID。包含未知非零 ID 的网络 Chunk 不构成合法客户端状态。
- `ModId` 是 `author.mod_name`。每段必须以小写 ASCII 字母或数字开头，后续只允许小写字母、数字、`_`、`-`。Resource local name 不含 `.`；完整同类型引用是 `author.mod_name.local`。
- `AnchorTree` 的合法状态是一个可达、无环、parent/children 双向一致且 child 不重复的单根树；允许的唯一空状态是 unrestricted root destruction 成功提交后的终态。

### 失效与 lifetime

- Rust 借用仍是最小 lifetime：容器 mutation 后不得保留此前借出的引用或 iterator。
- `Octree::insert`、`update`、`remove_value` 和 `remove_subtree` 成功后会清空 `optimized_octree`。调用方必须显式 rebuild 才能再次依赖 compact cache；失败不清空 cache。
- `AnchorTree::DestructionPlan` 只适用于产生它的同一棵、同一 version 的树。任意成功 `insert_child` 或 `apply_plan` 都使旧 plan 失效；提交旧 plan 返回 `PlanDoesNotMatchTree`，不部分应用。
- `RequestCall` 只拥有一次响应的 receiver。`wait(self)` 消耗 handle；`try_result(&self)` 返回 `None` 时可能是“尚无结果”或“响应端已断开”，当前 API 不区分两者。
- `CrossbeamThreadPipe` 和 endpoint 的 `clone` 共享同一对 channel，不创建隔离队列。只有相关方向的所有 sender 都 drop 后，阻塞 receive 才以 `None` 表示断开。
- `Chunk` 的 derived SVO/primitive snapshot 通过 `Arc` 共享。后续 voxel mutation 使用 copy-on-write 或重建 editable state，已有 snapshot 保持其创建时内容，不自动跟随更新。
- Mod Resource 在初始化加载完成后按当前模型不可变；没有 runtime registry reload 或热替换契约。

### 结构与 mutation

- `Octree::remove_value` 与 `remove_subtree` 都删除目标的完整子树；前者只归还根 value，后者归还 detached `Node`。root 不能删除。
- `BreadthFirstLosslessSvo` 保留每个有效最细坐标的 voxel 值，不保留源 Octree 拓扑。uniform region 会折叠，等于父继承值的 child 可被省略；调用方只能依赖 voxel-equivalence。
- `Chunk::set_voxel` 对越界 chunk-local 坐标返回 `false`。返回 `false` 也可能表示值未改变；成功改变会使 derived cache 失效并以 wrapping arithmetic 推进 revision。
- `LocalCoordinate::apply_voxels` 逐项立即修改 authoritative voxel state，但只在批次结束时重建一次 center of mass；返回值表示至少一项改变。它不是遇错回滚的 transaction，因为输入 item 本身没有可返回的业务错误。
- Lifecycle destruction 分为纯计算的 `plan_destruction` 和原子提交的 `apply_plan`。提交前 outcome 不可见；成功提交同时完成 reparent、删除和 root 更新。
- ECS component 写入通常到对应 Bevy schedule 才由下游观察。`LocalCoordinateTransform` 在 `PostUpdate`、Bevy transform propagation 之前复制到 engine `Transform`；写入该 component 不代表 `GlobalTransform` 已在同一语句后更新。
- Resource Type Loading Phase 的目标语义是同一类型跨全部 Mod 全有或全无发布；依赖类型只在前置 phase 全部成功后开始。具体 adapter 尚未全部实现 declaration 级多错误聚合，调用方不能假定所有错误都会一次返回。

### 失败与原子性

- `Registry::register` 遇到重复 key 时返回原始 key 和被拒绝的 definition；已有 entry 不被替换。
- `LoadedMods::discover` 忽略没有 regular `manifest.toml` 的直接子目录；一旦 manifest 存在，I/O、parse、重复 ID、缺依赖或 cycle 都使整个 discovery 失败，不发布 partial `LoadedMods`。
- `resolve_resource_reference` 不按 dependency 顺序搜索。local reference 只查 owner；完整引用必须位于 owner 显式声明的 transitive dependency closure 内。
- `RequestResponseIo::submit` 和网络 transport admission 都是非阻塞准入。有界队列满时返回/记录 saturated，而不是等待容量；channel 关闭与容量满是不同失败。
- JSON command 在进入共享 request queue 前受 64 KiB serialized-size 限制。超限返回 `InputTooLarge`，queue 不发生部分写入。
- 网络 frame 是 4 字节大端长度前缀加 postcard payload，payload 上限 64 KiB。编码过大、解码失败、短读/写和 I/O 错误都通过 `ProtocolError` 返回。
- `frame::write` 不提供跨底层 I/O 错误的回滚：长度前缀或 payload 可能已经部分写入。失败后该 stream 不应被当成仍位于 frame boundary。
- `roundo_user_config::save_config` 先完成序列化，再直接覆盖目标文件，但文件写入不是事务；I/O 失败可能留下截断或部分写入的旧配置。
- 对外 API 的 `expect` / `debug_assert` 只用于实现内部已经建立的不变量；它们不是调用者可用来处理无效输入的失败通道。若公开输入能触发 panic，必须在该 item 的 `# Panics` 中单独记录。

### 顺序与确定性

- `Registry` 基于 `HashMap`；`iter()` 顺序明确为不可依赖。需要 wire、hash、错误聚合或测试稳定顺序时，调用方必须显式排序。
- `LoadedMods`、Mod dependency closure 与 Resource Name 使用 `BTreeMap` / `BTreeSet`，迭代按 canonical `ModId` 排序。candidate winner 先比较 `override_priority`，再比较 canonical `ModId`；最大者胜出。
- Octree prefix/infix/postfix traversal 按 child slot 顺序；compact SVO 采用 breadth-first，显式 children 按 octant 0..7 packing。
- `AnchorTree::children` 保持 insertion order；destruction outcome 由该稳定 traversal 产生。传入 explicit target 的迭代顺序不可用于改变结果。
- 单个 QUIC stream 内可依赖其 transport 顺序。`Stream0` 与 `Stream1` 之间没有跨流顺序或因果交付保证；需要因果关系时必须在协议中用 identity/version 显式表达。
- Presence snapshot 的当前服务端路径按 `PlayerId` 排序。除非对应 item 保留此契约，调用方不应把其他 ECS query、HashMap 或异步 worker completion order 当作稳定顺序。
- PCG、资源候选裁决和 fingerprint 输入必须先 canonicalize 再 hash/serialize；线程完成先后不能进入结果身份。

### 并发、阻塞与 side effects

- `CrossbeamThreadPipe` 使用两个独立的 unbounded MPMC channel。`try_send` / `try_receive` 不阻塞；`receive` 阻塞当前 OS 线程；unbounded send 没有背压，调用方必须在更高层设置预算。
- `RequestResponsePipe` 使用 bounded MPMC request queue 和每请求一个 capacity-1 response channel。`submit` 不阻塞，`RequestCall::wait` 阻塞当前 OS 线程，`ResponseSender::respond` 在 caller 丢弃 response receiver 时归还未发送的 response。
- 网络 runtime 运行在独立 Tokio runtime 线程。应用 outbound queue 有界且按逻辑 stream 分容量；Stream0 默认 1024、Stream1 默认 256。准入成功只表示进入本地 queue，不表示 peer 已处理。
- client/server hook 可在 network runtime 上被调用；实现应快速返回或把工作转交给已有 IPC，不能把 ECS world 的线程亲和对象带入 network task。
- QUIC read/write task 必须独立拥有各自方向，避免 `tokio::select!` 因发送分支完成而取消尚未完成的 read future。
- `probe_quic_endpoint` 创建 current-thread Tokio runtime、执行 DNS/TLS/QUIC I/O、在 caller 线程阻塞到成功、错误或 timeout；成功仅证明可完成 QUIC dial，不加入游戏 session。
- `LoadedMods::discover` 执行同步 filesystem I/O；`read_only_svo`、center-of-mass rebuild 和 snapshot materialization 可能扫描整个 Chunk/树并分配。

### 单位与坐标

- 代码当前没有独立的 m/cm/mm 换算层，因此物理长度属于项目约定而非 SI 保证。Local Coordinate 内一个 voxel edge 是 1 个 local unit；`LocalCoordinateTransform.scale` 决定该 local unit 映射到多少 world units。wire Player translation 与 Bevy world translation 都是 `[f32; 3]`，但不能当作 Local Coordinate voxel position 直接互换。
- voxel position 是 Local Coordinate 内的整数坐标；Chunk edge 当前由 `CHUNK_EDGE_LENGTH` 定义。拆分使用 Euclidean division/remainder，因此负 voxel 坐标会映射到负 Chunk，chunk-local 坐标仍落在 `0..CHUNK_EDGE_LENGTH`。
- `ChunkId.coordinate` 是 Chunk 单位，不是 voxel 单位。`world.chunk_view_distance` 也以 Chunk 为单位。
- SVO coordinate 和 octant bit 顺序是 X 为 bit 0、Y 为 bit 1、Z 为 bit 2；每轴有效范围为 `0..2^maximum_depth`。
- quaternion wire layout 是 `[x, y, z, w]`，应在服务端校验 finite/non-zero 并归一化后再进入权威 rotation。
- LOD 距离使用 camera Chunk 到目标 Chunk 的平方欧氏距离。阈值也保持平方形式；不得把显存压力、frustum visibility 或 build backlog 混入 LOD identity。

## `unsafe` 契约

当前 Rust `unsafe` 集中在 Windows/WebView 平台 adapter。每个 FFI site 的局部注释应至少确认：

1. COM interface/Win32 handle 在调用期间有效；
2. 调用发生在平台要求的 UI/COM apartment 线程；
3. out parameter 在读取前已由成功返回初始化；
4. event callback 捕获值不会活过其 owner；
5. HRESULT/BOOL 失败在越过边界前转换为 Rust error 或显式 fallback。

安全审查以具体 `unsafe` block 为单位；“Windows API 需要 unsafe”不是充分的 safety contract。
