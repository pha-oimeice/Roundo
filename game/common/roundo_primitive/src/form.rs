//! 这里只预定义了Form，但不定义Form具体是什么。
//! Form的存在性需要预先注册。

use bevy::prelude::Resource;
use std::collections::HashSet;

#[derive(Resource, Debug, Clone)]
pub struct FormRegistry {
    registry: HashSet<String>,
}
