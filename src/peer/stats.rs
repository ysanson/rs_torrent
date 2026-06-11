use std::net::SocketAddr;

use crate::peer::state::BlockInfo;

pub(super) const BATCH_SIZE: usize = 16;

/// Statistics for pipelining performance
#[derive(Debug, Clone, Default)]
pub struct PipelineStats {
    pub total_requests_sent: u64,
    pub total_responses_received: u64,
    pub total_timeouts: u64,
    pub max_pipeline_depth_seen: usize,
    pub average_response_time_ms: f64,
}

/// Performance cache to eliminate allocations in hot paths (one instance per peer)
#[derive(Debug)]
pub struct PerformanceCache {
    pub bitfield_buffer: Vec<bool>,
    pub block_batch: Vec<BlockInfo>,
    /// Starting piece index for block picking, derived from peer address to spread work
    pub piece_offset: usize,
}

impl PerformanceCache {
    pub fn new_for_peer(total_pieces: usize, addr: &SocketAddr) -> Self {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        addr.hash(&mut hasher);
        let piece_offset = if total_pieces > 0 {
            (hasher.finish() as usize) % total_pieces
        } else {
            0
        };
        Self {
            bitfield_buffer: Vec::with_capacity(total_pieces),
            block_batch: Vec::with_capacity(BATCH_SIZE),
            piece_offset,
        }
    }

    pub fn clear(&mut self) {
        self.bitfield_buffer.clear();
        self.block_batch.clear();
    }
}
