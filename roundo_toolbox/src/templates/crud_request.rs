/// - K is the type of key \
/// All enum variants require a key
/// - V is the type of value \
/// Write operations require a value
#[derive(Debug, Clone)]
pub enum CRUDRequest<K, V> {
    Create { key: K, value: V },
    Retrieve { key: K },
    Update { key: K, value: V },
    Delete { key: K },
}
