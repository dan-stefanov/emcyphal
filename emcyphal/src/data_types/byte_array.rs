use emcyphal_encoding::{
    BufferType, DataType, Deserialize, DeserializeError, ReadCursor, Serialize, StaticBuffer,
    WriteCursor,
};
use heapless::Vec;

///  A simple (de)serializable type for tests and examples
///
/// Compatible with `uavcan.primitive.array.Natural8.1.0`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteArray {
    pub bytes: Vec<u8, 256>,
}

impl DataType for ByteArray {
    /// This type is sealed.
    const EXTENT_BYTES: Option<u32> = None;
}

impl Deserialize for ByteArray {
    fn deserialize(cursor: &mut ReadCursor<'_>) -> Result<Self, DeserializeError>
    where
        Self: Sized,
    {
        let length = usize::from(cursor.read_aligned_u16());
        if length <= 256 {
            let mut bytes = Vec::new();
            unwrap!(bytes.resize_default(length));
            cursor.read_bytes(&mut bytes);

            Ok(Self { bytes })
        } else {
            return Err(DeserializeError::ArrayLength);
        }
    }
}

impl Serialize for ByteArray {
    fn size_bits(&self) -> usize {
        16 + self.bytes.len() * 8
    }

    fn serialize(&self, cursor: &mut WriteCursor<'_>) {
        cursor.write_aligned_u16(unwrap!(self.bytes.len().try_into()));
        cursor.write_aligned_bytes(&self.bytes);
    }
}

impl BufferType for ByteArray {
    type Buffer = StaticBuffer<258>;
}
