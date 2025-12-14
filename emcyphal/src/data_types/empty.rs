use emcyphal_encoding::{
    BufferType, DataType, Deserialize, DeserializeError, ReadCursor, Serialize, StaticBuffer,
    WriteCursor,
};

/// `uavcan.primitive.Empty.1.0`
///
/// Fixed size 0 bytes.
/// Can be used to retrieve meta data, e.g., from loop-back transfers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Empty {}

impl DataType for Empty {
    /// This type is sealed.
    const EXTENT_BYTES: Option<u32> = None;
}

impl Deserialize for Empty {
    fn deserialize(_cursor: &mut ReadCursor<'_>) -> Result<Self, DeserializeError>
    where
        Self: Sized,
    {
        Ok(Self {})
    }
}

impl Serialize for Empty {
    fn size_bits(&self) -> usize {
        0
    }

    fn serialize(&self, _cursor: &mut WriteCursor<'_>) {}
}

impl BufferType for Empty {
    type Buffer = StaticBuffer<0>;
}
