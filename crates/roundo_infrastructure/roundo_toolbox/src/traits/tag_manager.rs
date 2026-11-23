/// Provides typed lookup and replacement for tag-associated values.
pub trait TagManager<T, V>
where
    T: Copy,
    V: Copy,
{
    fn get_tag(&self, tag: T) -> V;
    fn set_tag(&mut self, tag: T, value: V);
}
