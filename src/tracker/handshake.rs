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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::{PSTR, PSTR_LEN};

    #[test]
    fn test_handshake_serialize() {
        let infohash = [1u8; 20];
        let peer_id = [2u8; 20];
        let handshake = Handshake { infohash, peer_id };

        let serialized = handshake.serialize();

        assert_eq!(serialized.len(), 68);
        assert_eq!(serialized[0], PSTR_LEN);
        assert_eq!(&serialized[1..20], PSTR.as_bytes());
        // Check reserved bytes are zero
        assert_eq!(&serialized[20..28], &[0u8; 8]);
        assert_eq!(&serialized[28..48], &infohash);
        assert_eq!(&serialized[48..68], &peer_id);
    }

    #[test]
    fn test_handshake_deserialize_valid() {
        let mut buf = [0u8; 68];
        buf[0] = PSTR_LEN;
        buf[1..20].copy_from_slice(PSTR.as_bytes());
        // buf[20..28] reserved bytes remain zero
        let infohash = [3u8; 20];
        let peer_id = [4u8; 20];
        buf[28..48].copy_from_slice(&infohash);
        buf[48..68].copy_from_slice(&peer_id);

        let handshake = Handshake::deserialize(&buf).unwrap();

        assert_eq!(handshake.infohash, infohash);
        assert_eq!(handshake.peer_id, peer_id);
    }

    #[test]
    fn test_handshake_deserialize_fixed_valid() {
        let mut buf = [0u8; 68];
        buf[0] = PSTR_LEN;
        buf[1..20].copy_from_slice(PSTR.as_bytes());
        let infohash = [5u8; 20];
        let peer_id = [6u8; 20];
        buf[28..48].copy_from_slice(&infohash);
        buf[48..68].copy_from_slice(&peer_id);

        let handshake = Handshake::deserialize_fixed(buf).unwrap();

        assert_eq!(handshake.infohash, infohash);
        assert_eq!(handshake.peer_id, peer_id);
    }

    #[test]
    fn test_handshake_deserialize_invalid_length() {
        let buf = [0u8; 67]; // Wrong length
        let result = Handshake::deserialize(&buf);
        assert!(result.is_none());
    }

    #[test]
    fn test_handshake_deserialize_invalid_pstr_len() {
        let mut buf = [0u8; 68];
        buf[0] = PSTR_LEN + 1; // Wrong protocol string length
        buf[1..20].copy_from_slice(PSTR.as_bytes());

        let result = Handshake::deserialize(&buf);
        assert!(result.is_none());
    }

    #[test]
    fn test_handshake_deserialize_invalid_pstr() {
        let mut buf = [0u8; 68];
        buf[0] = PSTR_LEN;
        buf[1..20].copy_from_slice(b"Invalid protocol!!X"); // Wrong protocol string (19 bytes)

        let result = Handshake::deserialize(&buf);
        assert!(result.is_none());
    }

    #[test]
    fn test_handshake_serialize_deserialize_roundtrip() {
        let infohash = [7u8; 20];
        let peer_id = [8u8; 20];
        let original = Handshake { infohash, peer_id };

        let serialized = original.serialize();
        let deserialized = Handshake::deserialize(&serialized).unwrap();

        assert_eq!(deserialized.infohash, original.infohash);
        assert_eq!(deserialized.peer_id, original.peer_id);
    }

    #[test]
    fn test_handshake_different_infohashes() {
        let infohash1 = [9u8; 20];
        let infohash2 = [10u8; 20];
        let peer_id = [11u8; 20];

        let handshake1 = Handshake {
            infohash: infohash1,
            peer_id,
        };
        let handshake2 = Handshake {
            infohash: infohash2,
            peer_id,
        };

        let serialized1 = handshake1.serialize();
        let serialized2 = handshake2.serialize();

        assert_ne!(serialized1, serialized2);
        assert_eq!(&serialized1[28..48], &infohash1);
        assert_eq!(&serialized2[28..48], &infohash2);
    }

    #[test]
    fn test_handshake_different_peer_ids() {
        let infohash = [12u8; 20];
        let peer_id1 = [13u8; 20];
        let peer_id2 = [14u8; 20];

        let handshake1 = Handshake {
            infohash,
            peer_id: peer_id1,
        };
        let handshake2 = Handshake {
            infohash,
            peer_id: peer_id2,
        };

        let serialized1 = handshake1.serialize();
        let serialized2 = handshake2.serialize();

        assert_ne!(serialized1, serialized2);
        assert_eq!(&serialized1[48..68], &peer_id1);
        assert_eq!(&serialized2[48..68], &peer_id2);
    }

    #[test]
    fn test_handshake_reserved_bytes_are_zero() {
        let infohash = [15u8; 20];
        let peer_id = [16u8; 20];
        let handshake = Handshake { infohash, peer_id };

        let serialized = handshake.serialize();

        // Check that reserved bytes (positions 20-27) are all zero
        for i in 20..28 {
            assert_eq!(serialized[i], 0);
        }
    }

    #[test]
    fn test_handshake_deserialize_with_non_zero_reserved_bytes() {
        let mut buf = [0u8; 68];
        buf[0] = PSTR_LEN;
        buf[1..20].copy_from_slice(PSTR.as_bytes());
        // Set reserved bytes to non-zero values
        buf[20..28].fill(0xFF);
        let infohash = [17u8; 20];
        let peer_id = [18u8; 20];
        buf[28..48].copy_from_slice(&infohash);
        buf[48..68].copy_from_slice(&peer_id);

        // Should still deserialize successfully (reserved bytes are ignored)
        let handshake = Handshake::deserialize(&buf).unwrap();
        assert_eq!(handshake.infohash, infohash);
        assert_eq!(handshake.peer_id, peer_id);
    }
}
