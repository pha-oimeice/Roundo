use crate::graph::Vertex;
use bevy::prelude::*;

/// ## 局部世界
/// 数据：
/// - 每个节点都需要知道自己隶属于哪张图。
/// - 每个节点都需要知道自己的父图节点引用协议。
struct _Documentation;

#[derive(Component, Debug)]
#[require(Vertex)]
pub struct WorldGraphVertex {}
