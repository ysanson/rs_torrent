#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageId {
    Choke = 0,
    Unchoke = 1,
    Interested = 2,
    NotInterested = 3,
    Have = 4,
    Bitfield = 5,
    Request = 6,
    Piece = 7,
    Cancel = 8,
}

impl TryFrom<u8> for MessageId {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Choke),
            1 => Ok(Self::Unchoke),
            2 => Ok(Self::Interested),
            3 => Ok(Self::NotInterested),
            4 => Ok(Self::Have),
            5 => Ok(Self::Bitfield),
            6 => Ok(Self::Request),
            7 => Ok(Self::Piece),
            8 => Ok(Self::Cancel),
            _ => Err(()),
        }
    }
}

pub struct Message {
    pub kind: MessageId,
    pub payload: Vec<u8>,
}

impl Message {
    pub fn serialize(&self) -> Vec<u8> {
        let total_len = 1 + self.payload.len(); // 1 byte for ID
        let mut buf = Vec::with_capacity(4 + total_len);

        buf.extend_from_slice(&(total_len as u32).to_be_bytes()); // 4-byte length
        buf.push(self.kind as u8); // 1-byte message ID
        buf.extend_from_slice(&self.payload); // payload

        buf
    }

    pub fn deserialize(buf: &[u8]) -> Option<Self> {
        if buf.len() < 5 {
            return None; // must be at least 4 (length) + 1 (ID)
        }

        let len = u32::from_be_bytes(buf[0..4].try_into().ok()?) as usize;
        if buf.len() < 4 + len {
            return None; // not enough bytes
        }

        let kind = MessageId::try_from(buf[4]).ok()?;
        let payload = buf[5..4 + len].to_vec();

        Some(Self { kind, payload })
    }
}

#[derive(Debug, Clone)]
pub struct Bitfield {
    pub bits: Vec<u8>, // raw bytes
}

impl Bitfield {
    pub fn has_piece(&self, index: usize) -> bool {
        let byte = index / 8;
        let bit = 7 - (index % 8); // MSB first
        if byte >= self.bits.len() {
            return false;
        }
        self.bits[byte] & (1 << bit) != 0
    }

    pub fn set_piece(&mut self, index: usize) {
        let byte = index / 8;
        let bit = 7 - (index % 8);
        if byte < self.bits.len() {
            self.bits[byte] |= 1 << bit;
        }
    }

    pub fn from_piece_count(count: usize) -> Self {
        let num_bytes = count.div_ceil(8);
        Bitfield {
            bits: vec![0; num_bytes],
        }
    }
}

impl From<Bitfield> for Message {
    fn from(b: Bitfield) -> Self {
        Message {
            kind: MessageId::Bitfield,
            payload: b.bits,
        }
    }
}

impl TryFrom<Message> for Bitfield {
    type Error = &'static str;

    fn try_from(msg: Message) -> Result<Self, Self::Error> {
        if msg.kind != MessageId::Bitfield {
            return Err("Not a bitfield message");
        }
        Ok(Bitfield { bits: msg.payload })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_id_try_from_valid() {
        assert_eq!(MessageId::try_from(0), Ok(MessageId::Choke));
        assert_eq!(MessageId::try_from(1), Ok(MessageId::Unchoke));
        assert_eq!(MessageId::try_from(2), Ok(MessageId::Interested));
        assert_eq!(MessageId::try_from(3), Ok(MessageId::NotInterested));
        assert_eq!(MessageId::try_from(4), Ok(MessageId::Have));
        assert_eq!(MessageId::try_from(5), Ok(MessageId::Bitfield));
        assert_eq!(MessageId::try_from(6), Ok(MessageId::Request));
        assert_eq!(MessageId::try_from(7), Ok(MessageId::Piece));
        assert_eq!(MessageId::try_from(8), Ok(MessageId::Cancel));
    }

    #[test]
    fn test_message_id_try_from_invalid() {
        assert_eq!(MessageId::try_from(9), Err(()));
        assert_eq!(MessageId::try_from(255), Err(()));
    }

    #[test]
    fn test_message_id_values() {
        assert_eq!(MessageId::Choke as u8, 0);
        assert_eq!(MessageId::Unchoke as u8, 1);
        assert_eq!(MessageId::Interested as u8, 2);
        assert_eq!(MessageId::NotInterested as u8, 3);
        assert_eq!(MessageId::Have as u8, 4);
        assert_eq!(MessageId::Bitfield as u8, 5);
        assert_eq!(MessageId::Request as u8, 6);
        assert_eq!(MessageId::Piece as u8, 7);
        assert_eq!(MessageId::Cancel as u8, 8);
    }

    #[test]
    fn test_message_serialize_no_payload() {
        let msg = Message {
            kind: MessageId::Choke,
            payload: vec![],
        };
        let serialized = msg.serialize();

        assert_eq!(serialized.len(), 5); // 4 bytes length + 1 byte ID
        assert_eq!(serialized[0..4], [0, 0, 0, 1]); // length = 1
        assert_eq!(serialized[4], 0); // MessageId::Choke
    }

    #[test]
    fn test_message_serialize_with_payload() {
        let msg = Message {
            kind: MessageId::Have,
            payload: vec![0x12, 0x34, 0x56, 0x78],
        };
        let serialized = msg.serialize();

        assert_eq!(serialized.len(), 9); // 4 bytes length + 1 byte ID + 4 bytes payload
        assert_eq!(serialized[0..4], [0, 0, 0, 5]); // length = 5 (1 + 4)
        assert_eq!(serialized[4], 4); // MessageId::Have
        assert_eq!(serialized[5..9], [0x12, 0x34, 0x56, 0x78]);
    }

    #[test]
    fn test_message_deserialize_valid() {
        let buf = vec![0, 0, 0, 3, 2, 0xFF, 0xAA]; // length=3, MessageId::Interested, payload=[0xFF, 0xAA]
        let msg = Message::deserialize(&buf).unwrap();

        assert_eq!(msg.kind, MessageId::Interested);
        assert_eq!(msg.payload, vec![0xFF, 0xAA]);
    }

    #[test]
    fn test_message_deserialize_no_payload() {
        let buf = vec![0, 0, 0, 1, 1]; // length=1, MessageId::Unchoke, no payload
        let msg = Message::deserialize(&buf).unwrap();

        assert_eq!(msg.kind, MessageId::Unchoke);
        assert_eq!(msg.payload, Vec::<u8>::new());
    }

    #[test]
    fn test_message_deserialize_too_short() {
        let buf = vec![0, 0, 0]; // Less than 5 bytes
        assert!(Message::deserialize(&buf).is_none());
    }

    #[test]
    fn test_message_deserialize_invalid_message_id() {
        let buf = vec![0, 0, 0, 1, 99]; // Invalid message ID
        assert!(Message::deserialize(&buf).is_none());
    }

    #[test]
    fn test_message_deserialize_incomplete_payload() {
        let buf = vec![0, 0, 0, 5, 4, 0x12]; // Says length=5 but only has 2 bytes after ID
        assert!(Message::deserialize(&buf).is_none());
    }

    #[test]
    fn test_message_serialize_deserialize_roundtrip() {
        let original = Message {
            kind: MessageId::Piece,
            payload: vec![1, 2, 3, 4, 5],
        };
        let serialized = original.serialize();
        let deserialized = Message::deserialize(&serialized).unwrap();

        assert_eq!(deserialized.kind, original.kind);
        assert_eq!(deserialized.payload, original.payload);
    }

    #[test]
    fn test_bitfield_has_piece_basic() {
        let bitfield = Bitfield {
            bits: vec![0b10000000, 0b00000001], // First bit and last bit of second byte set
        };

        assert!(bitfield.has_piece(0)); // First bit set
        assert!(!bitfield.has_piece(1)); // Second bit not set
        assert!(!bitfield.has_piece(7)); // Last bit of first byte not set
        assert!(!bitfield.has_piece(8)); // First bit of second byte not set
        assert!(bitfield.has_piece(15)); // Last bit of second byte set
    }

    #[test]
    fn test_bitfield_has_piece_out_of_bounds() {
        let bitfield = Bitfield { bits: vec![0xFF] };

        assert!(!bitfield.has_piece(8)); // Beyond available bits
        assert!(!bitfield.has_piece(100)); // Way beyond available bits
    }

    #[test]
    fn test_bitfield_set_piece() {
        let mut bitfield = Bitfield {
            bits: vec![0x00, 0x00],
        };

        bitfield.set_piece(0);
        assert!(bitfield.has_piece(0));
        assert_eq!(bitfield.bits[0], 0b10000000);

        bitfield.set_piece(7);
        assert!(bitfield.has_piece(7));
        assert_eq!(bitfield.bits[0], 0b10000001);

        bitfield.set_piece(8);
        assert!(bitfield.has_piece(8));
        assert_eq!(bitfield.bits[1], 0b10000000);
    }

    #[test]
    fn test_bitfield_set_piece_out_of_bounds() {
        let mut bitfield = Bitfield { bits: vec![0x00] };

        // Should not panic when setting out of bounds
        bitfield.set_piece(8);
        bitfield.set_piece(100);
        assert_eq!(bitfield.bits[0], 0x00); // Should remain unchanged
    }

    #[test]
    fn test_bitfield_from_piece_count() {
        let bitfield = Bitfield::from_piece_count(7);
        assert_eq!(bitfield.bits.len(), 1); // 7 pieces fit in 1 byte

        let bitfield = Bitfield::from_piece_count(8);
        assert_eq!(bitfield.bits.len(), 1); // 8 pieces fit in 1 byte

        let bitfield = Bitfield::from_piece_count(9);
        assert_eq!(bitfield.bits.len(), 2); // 9 pieces need 2 bytes

        let bitfield = Bitfield::from_piece_count(0);
        assert_eq!(bitfield.bits.len(), 0); // 0 pieces need 0 bytes
    }

    #[test]
    fn test_bitfield_all_bits_operations() {
        let mut bitfield = Bitfield::from_piece_count(16);

        // Set all bits
        for i in 0..16 {
            bitfield.set_piece(i);
        }

        // Check all bits are set
        for i in 0..16 {
            assert!(bitfield.has_piece(i));
        }

        assert_eq!(bitfield.bits, vec![0xFF, 0xFF]);
    }

    #[test]
    fn test_bitfield_to_message() {
        let bitfield = Bitfield {
            bits: vec![0xAB, 0xCD],
        };
        let message: Message = bitfield.into();

        assert_eq!(message.kind, MessageId::Bitfield);
        assert_eq!(message.payload, vec![0xAB, 0xCD]);
    }

    #[test]
    fn test_message_to_bitfield_valid() {
        let message = Message {
            kind: MessageId::Bitfield,
            payload: vec![0x12, 0x34],
        };
        let bitfield: Bitfield = message.try_into().unwrap();

        assert_eq!(bitfield.bits, vec![0x12, 0x34]);
    }

    #[test]
    fn test_message_to_bitfield_invalid() {
        let message = Message {
            kind: MessageId::Have,
            payload: vec![0x12, 0x34],
        };
        let result: Result<Bitfield, _> = message.try_into();

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Not a bitfield message");
    }

    #[test]
    fn test_bitfield_message_roundtrip() {
        let original = Bitfield {
            bits: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        let message: Message = original.clone().into();
        let recovered: Bitfield = message.try_into().unwrap();

        assert_eq!(recovered.bits, original.bits);
    }

    #[test]
    fn test_message_id_equality() {
        assert_eq!(MessageId::Choke, MessageId::Choke);
        assert_ne!(MessageId::Choke, MessageId::Unchoke);
    }

    #[test]
    fn test_large_message_payload() {
        let large_payload = vec![0x42; 1000];
        let msg = Message {
            kind: MessageId::Piece,
            payload: large_payload.clone(),
        };
        let serialized = msg.serialize();
        let deserialized = Message::deserialize(&serialized).unwrap();

        assert_eq!(deserialized.kind, MessageId::Piece);
        assert_eq!(deserialized.payload, large_payload);
    }
}
