//! UI Lifecycle Tree 的纯数据 core。
//!
//! 此 crate 只提供 UI lifecycle adapter 共享的所有权树与原子销毁事务。
//! 它不依赖 Bevy、ECS 调度或资源清理；平台 adapter 负责把提交结果映射为
//! WebView 等外部资源操作。
//!
//! 这保持 ADR-0003 的 seam：页面和平台 adapter 不拥有 Lifecycle Tree，
//! 服务端 ECS 实验也不参与 UI 生命周期。

mod lifecycle_tree;

pub use lifecycle_tree::{
    AnchorTree, DestructionOutcome, DestructionPlan, Reparented, TreeError, TreeInvariantError,
};
