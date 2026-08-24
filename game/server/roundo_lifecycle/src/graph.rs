use bevy::prelude::*;
use std::collections::HashSet;

#[derive(Component, Debug, Default)]
pub struct Vertex {
    pub adjacency_list: HashSet<Entity>,
}
#[derive(Debug)]
pub enum GraphErrorCode {
    Success,
    EdgeAlreadyExist,
    EdgeDoesNotExist,
}

/// 创建一条指向entity的边，不保证target存在
pub fn add_edge(v_0: &mut Vertex, v_1: Entity) -> GraphErrorCode {
    if v_0.adjacency_list.contains(&v_1) {
        return GraphErrorCode::EdgeAlreadyExist;
    }
    v_0.adjacency_list.insert(v_1);
    GraphErrorCode::Success
}

/// 删除一条已存在的边
pub fn remove_edge(v_0: &mut Vertex, v_1: Entity) -> GraphErrorCode {
    if !v_0.adjacency_list.contains(&v_1) {
        return GraphErrorCode::EdgeDoesNotExist;
    }
    v_0.adjacency_list.remove(&v_1);
    GraphErrorCode::Success
}
