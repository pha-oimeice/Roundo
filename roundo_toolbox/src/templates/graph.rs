use std::collections::HashMap;

pub struct Graph<V, E>
where
    V: Copy + PartialEq + Eq + std::hash::Hash,
    E: Copy + PartialEq + Eq + std::hash::Hash,
{
    pub vertices: Vec<V>,
    pub adjacency_list: HashMap<V, Vec<(V, E)>>,
}

impl<V, E> Graph<V, E>
where
    V: Copy + PartialEq + Eq + std::hash::Hash,
    E: Copy + PartialEq + Eq + std::hash::Hash,
{
}
