use rustc_hash::FxHashMap;
use std::collections::VecDeque;
use std::time::Duration;

use crate::peer::Peer;
use crate::peer::message::{Bitfield, Message, PieceRequest};
use crate::peer::state::BlockInfo;

pub(super) const MAX_PIPELINE_DEPTH: usize = 10;
pub(super) const MAX_UPLOAD_QUEUE_DEPTH: usize = 16;
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Represents the state of a connection to a peer
#[derive(Debug, Clone)]
pub struct PeerConnection {
    pub peer: Peer,
    pub am_choking: bool,
    pub am_interested: bool,
    pub peer_choking: bool,
    pub peer_interested: bool,
    pub bitfield: Option<Bitfield>,
    pub pending_requests: FxHashMap<BlockInfo, std::time::Instant>, // Blocks we've requested with timestamps
    pub last_request_time: Option<std::time::Instant>,
    pub pending_uploads: VecDeque<PieceRequest>,
    /// The ut_metadata extension ID the *peer* assigned in their extension handshake.
    /// Populated when we receive and parse the peer's `Extended` handshake (msg ID 0).
    /// Required to address metadata data/reject responses back to the peer.
    pub peer_ut_metadata_id: Option<u8>,
    pub pending_metadata_responses: VecDeque<Message>,
}

impl PeerConnection {
    pub fn new(peer: Peer) -> Self {
        Self {
            peer,
            am_choking: true,
            am_interested: false,
            peer_choking: true,
            peer_interested: false,
            bitfield: None,
            pending_requests: FxHashMap::default(),
            last_request_time: None,
            pending_uploads: VecDeque::new(),
            peer_ut_metadata_id: None,
            pending_metadata_responses: VecDeque::new(),
        }
    }

    pub fn has_piece(&self, index: usize) -> bool {
        self.bitfield
            .as_ref()
            .map(|bf| bf.has_piece(index))
            .unwrap_or(false)
    }

    pub fn can_download(&self) -> bool {
        !self.peer_choking && self.am_interested
    }

    pub fn can_pipeline_request(&self) -> bool {
        self.can_download() && self.pending_requests.len() < MAX_PIPELINE_DEPTH
    }

    pub fn add_pending_request(&mut self, block_info: BlockInfo) {
        let now = std::time::Instant::now();
        self.pending_requests.insert(block_info, now);
        self.last_request_time = Some(now);
    }

    pub fn remove_pending_request(&mut self, block_info: &BlockInfo) -> bool {
        self.pending_requests.remove(block_info).is_some()
    }

    pub fn has_pending_requests(&self) -> bool {
        !self.pending_requests.is_empty()
    }

    pub fn get_timed_out_requests(&self) -> Vec<BlockInfo> {
        let now = std::time::Instant::now();
        self.pending_requests
            .iter()
            .filter_map(|(block_info, timestamp)| {
                if now.duration_since(*timestamp) > REQUEST_TIMEOUT {
                    Some(block_info.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn clear_timed_out_requests(&mut self) -> Vec<BlockInfo> {
        let timed_out = self.get_timed_out_requests();
        for block_info in &timed_out {
            self.pending_requests.remove(block_info);
        }
        timed_out
    }

    pub fn can_queue_upload(&self) -> bool {
        self.pending_uploads.len() < MAX_UPLOAD_QUEUE_DEPTH
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::Peer;
    use crate::peer::state::BlockInfo;

    #[test]
    fn test_peer_connection_creation() {
        use std::net::Ipv4Addr;
        let peer = Peer {
            ip_addr: Ipv4Addr::new(192, 168, 1, 1),
            port: 6881,
        };
        let conn = PeerConnection::new(peer);
        assert!(conn.am_choking);
        assert!(!conn.am_interested);
        assert!(conn.peer_choking);
        assert!(!conn.peer_interested);
        assert!(conn.bitfield.is_none());
        assert!(conn.pending_requests.is_empty());
        assert!(conn.last_request_time.is_none());
    }

    #[test]
    fn test_peer_connection_pipelining() {
        use std::net::Ipv4Addr;
        let peer = Peer {
            ip_addr: Ipv4Addr::new(192, 168, 1, 1),
            port: 6881,
        };
        let mut conn = PeerConnection::new(peer);

        // Initially can't download because peer is choking
        assert!(!conn.can_download());
        assert!(!conn.can_pipeline_request());

        // Unchoke and set interested
        conn.peer_choking = false;
        conn.am_interested = true;
        assert!(conn.can_download());
        assert!(conn.can_pipeline_request());

        // Add requests up to pipeline depth
        for i in 0..MAX_PIPELINE_DEPTH {
            let block_info = BlockInfo {
                piece_index: i,
                offset: 0,
                length: 16384,
            };
            conn.add_pending_request(block_info);
        }

        // Should not be able to pipeline more
        assert!(!conn.can_pipeline_request());

        // Remove one request
        let block_info = BlockInfo {
            piece_index: 0,
            offset: 0,
            length: 16384,
        };
        assert!(conn.remove_pending_request(&block_info));
        assert!(conn.can_pipeline_request());
    }

    #[test]
    fn test_request_timeout_handling() {
        use std::net::Ipv4Addr;
        let peer = Peer {
            ip_addr: Ipv4Addr::new(192, 168, 1, 1),
            port: 6881,
        };
        let mut conn = PeerConnection::new(peer);

        // Add a request with manual timestamp (simulating old request)
        let block_info = BlockInfo {
            piece_index: 0,
            offset: 0,
            length: 16384,
        };
        let old_time = std::time::Instant::now() - Duration::from_secs(120); // 2 minutes ago
        conn.pending_requests.insert(block_info.clone(), old_time);

        // Check that it's detected as timed out
        let timed_out = conn.get_timed_out_requests();
        assert_eq!(timed_out.len(), 1);
        assert_eq!(timed_out[0], block_info);

        // Clear timed out requests
        let cleared = conn.clear_timed_out_requests();
        assert_eq!(cleared.len(), 1);
        assert!(conn.pending_requests.is_empty());
    }
}
