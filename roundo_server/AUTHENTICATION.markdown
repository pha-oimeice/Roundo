# 玩家身份与外部认证方向

状态：设计记录。本文描述删除 OAuth 遗留实现后的 seam；它不声明 Firebase 或其它认证 adapter 已经接入。

## 决策

Roundo 游戏代码不拥有 OAuth 登录流程，也不与 Google、Firebase 或某个 JWT issuer 耦合。

游戏只依赖认证结果：

1. 在允许客户端进入游戏 session 前，确认其身份已通过验证；
2. 得到可稳定比较的、认证供应商无关的玩家身份；
3. 将该身份映射到游戏自己的 `UserId`，并在连接期继续使用 `UserSession`、`ConnectionId` 与 `PlayerId`。

OAuth redirect、consent、authorization code、refresh token、JWT 获取与刷新、provider discovery、JWKS 和 provider-specific claims 都不属于游戏实现。

## 当前实现

当前服务器只开放 public session。`roundo_networking::ServerHooks` 的 `public_session` 和 `session_is_open` 仍是 session 建立 seam；wire contract 中的 `UserId`、`SessionId`、`UserSession` 与运行期 `ConnectionId`、`PlayerId` 继续保留。

public session 下所有连接共享一个 `UserId`/`SessionId`；服务器以自己签发的 `ConnectionId` 区分同时在线的连接，Presence 再为每个连接签发 `PlayerId`。因此当前只能提供连接期身份，不能提供跨重连的已认证玩家身份。

已删除的内容包括 Google login form、provider/provider-subject 数据模型、`roundo_user_authentications` 建表逻辑，以及未使用的 `jsonwebtoken`/`jwks` workspace 依赖。启动时会删除旧的 provider-account 映射表，避免遗留 schema 被误认为仍受支持。

## 未来 adapter seam

当需要持久、已认证的玩家身份时，在进程组合根接入外部认证 lib，并让 adapter 完成以下工作：

- 客户端 adapter 从认证 lib 获取或刷新凭据；游戏 UI 和网络层不实现 OAuth/JWT 获取流程。
- 服务端 adapter 把客户端提交的不透明凭据交给认证 lib 验证；游戏代码不解析 provider-specific token 或 claims。
- adapter 只向游戏返回 provider-neutral、稳定且可比较的 principal；组合根再将其解析为内部 `UserId`/`UserSession`。
- 验证失败只跨现有 session seam 表现为拒绝，不把 Google/Firebase 错误类型扩散到 wire contract、ECS 或领域 module。

如果未来凭据必须穿过 Roundo wire，应增加不透明 credential 类型及对应 hook，而不是增加 `GoogleToken`、`FirebaseJwt` 等供应商类型。凭据不得进入日志、ECS component、游戏存档或 provider-specific 数据表。

## 所有权

- 外部认证 lib：登录、凭据生命周期、签名/issuer/audience 校验及供应商细节。
- host adapter：调用认证 lib，并把 verified principal 映射为 Roundo 内部身份。
- `roundo_networking`：传递必要的不透明认证输入、维护 session 建立顺序，不解释凭据。
- 游戏领域 module：只消费 `UserId`、`UserSession`、`ConnectionId` 或 `PlayerId`。
- 游戏数据库：只保存游戏拥有的用户与 session 数据；外部账号映射由认证集成侧拥有。

在真实认证 lib 和至少一个 public/test adapter 同时存在前，不新增 provider 抽象层或 JWT utility crate。优先扩展现有 session establishment seam，避免建立没有变化点的浅 module。
