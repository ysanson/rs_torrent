use crate::tracker::{PSTR, PSTR_LEN};

#[derive(Debug)]
pub struct Handshake {
    pub infohash: [u8; 20],
    pub peer_id: [u8; 20],
}

impl Handshake {
    pub fn serialize(&self) -> [u8; 68] {
        let mut buf = [0u8; 68];
        buf[0] = PSTR_LEN;
        buf[1..20].copy_from_slice(PSTR.as_bytes());
        // buf[20..28] is already zero (reserved)
        buf[28..48].copy_from_slice(&self.infohash);
        buf[48..68].copy_from_slice(&self.peer_id);

        buf
    }

    pub fn deserialize(buf: &[u8]) -> Option<Self> {
        if buf.len() != 68 || buf[0] != PSTR_LEN {
            return None;
        }
        if &buf[1..20] != PSTR.as_bytes() {
            return None;
        }

        let infohash = <[u8; 20]>::try_from(&buf[28..48]).ok()?;
        let peer_id = <[u8; 20]>::try_from(&buf[48..68]).ok()?;

        Some(Self { infohash, peer_id })
    }

    pub fn deserialize_fixed(buf: [u8; 68]) -> Option<Self> {
        Self::deserialize(&buf)
    }
}
