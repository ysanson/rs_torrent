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
        let num_bytes = (count + 7) / 8;
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
