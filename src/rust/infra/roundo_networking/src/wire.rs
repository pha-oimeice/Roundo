use crate::{NetworkError, SharedEntityId};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Visitor};
use std::{
    any::TypeId,
    fmt,
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};

/// Same-build wire fingerprint of Rust's TypeId, not a persistent identity or
/// a registration-order index. No cross-build compatibility is promised.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct NetworkTypeId(u64);

impl NetworkTypeId {
    pub fn of<T: ?Sized + 'static>() -> Self {
        // Unlike RandomState, DefaultHasher::new uses the same initial state
        // in both endpoints. No access to TypeId's opaque layout is required.
        let mut hasher = DefaultHasher::new();
        TypeId::of::<T>().hash(&mut hasher);
        Self(hasher.finish())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UpdateKind {
    Component,
    Resource,
}

/// Owned encoded data. Cloning shares the allocation, not the source component.
#[derive(Clone, Debug)]
pub(crate) struct SharedBytes(pub Arc<Vec<u8>>);

impl Serialize for SharedBytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0.as_slice())
    }
}

impl<'de> Deserialize<'de> for SharedBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BytesVisitor;
        impl<'de> Visitor<'de> for BytesVisitor {
            type Value = SharedBytes;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("owned serialized network data")
            }
            fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                Ok(SharedBytes(Arc::new(value.to_vec())))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
                Ok(SharedBytes(Arc::new(value)))
            }
        }
        deserializer.deserialize_byte_buf(BytesVisitor)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct WireUpdate {
    pub type_id: NetworkTypeId,
    pub entity: Option<SharedEntityId>,
    pub data: SharedBytes,
}

impl WireUpdate {
    pub fn kind(&self) -> UpdateKind {
        if self.entity.is_some() {
            UpdateKind::Component
        } else {
            UpdateKind::Resource
        }
    }
}

/// Application-layer envelope containing encoded original data types, without
/// duplicating component/resource fields. QUIC framing is unchanged.
/// Use `SyncConnection` for both client connects and server accepts.
#[derive(Debug, crate::DataTransferObject)]
pub struct SyncBatch(pub(crate) Vec<WireUpdate>);

pub type SyncConnection = crate::Connection<SyncBatch, SyncBatch>;

/// Splits only to respect the connection's existing message limit, not an
/// application capacity budget. Entries remain in their submission order.
pub(crate) fn split_batches(
    updates: Vec<WireUpdate>,
    limit: usize,
) -> Result<Vec<SyncBatch>, NetworkError> {
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut entry_bytes = 0usize;
    for update in updates {
        let size = postcard::experimental::serialized_size(&update)?;
        let count = current
            .len()
            .checked_add(1)
            .ok_or(NetworkError::SizeOverflow)?;
        let bytes = entry_bytes
            .checked_add(size)
            .ok_or(NetworkError::SizeOverflow)?;
        let prefix = postcard::experimental::serialized_size(&count)?;
        if bytes
            .checked_add(prefix)
            .ok_or(NetworkError::SizeOverflow)?
            > limit
        {
            if current.is_empty() {
                return Err(NetworkError::MessageTooLarge);
            }
            batches.push(SyncBatch(std::mem::take(&mut current)));
            entry_bytes = 0;
        }
        // A single entry must also fit when the preceding batch was nonempty.
        let single_prefix = postcard::experimental::serialized_size(&1usize)?;
        if size
            .checked_add(single_prefix)
            .ok_or(NetworkError::SizeOverflow)?
            > limit
        {
            return Err(NetworkError::MessageTooLarge);
        }
        entry_bytes = entry_bytes
            .checked_add(size)
            .ok_or(NetworkError::SizeOverflow)?;
        current.push(update);
    }
    if !current.is_empty() {
        batches.push(SyncBatch(current));
    }
    Ok(batches)
}
