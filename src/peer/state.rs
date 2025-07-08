use std::collections::{HashMap, HashSet};

const PIECE_BLOCK_SIZE: usize = 16384; // 16KB blocks

/// Represents a block within a piece
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockInfo {
    pub piece_index: usize,
    pub offset: usize,
    pub length: usize,
}

/// Shared download state for all peer workers
#[derive(Debug)]
pub struct DownloadState {
    pub total_pieces: usize,
    pub piece_length: u32,
    pub info_hash: [u8; 20],
    pub pieces: Vec<Option<Vec<u8>>>, // `None` = not downloaded, `Some(data)` = done
    pub requested: HashSet<usize>,    // Which pieces are currently being downloaded
    pub piece_blocks: HashMap<usize, Vec<Option<Vec<u8>>>>, // Blocks within each piece
    pub requested_blocks: HashSet<BlockInfo>, // Which blocks are currently being requested
}

impl DownloadState {
    pub fn new(total_pieces: usize, piece_length: u32, info_hash: [u8; 20]) -> Self {
        Self {
            total_pieces,
            piece_length,
            info_hash,
            pieces: vec![None; total_pieces],
            requested: HashSet::new(),
            piece_blocks: HashMap::new(),
            requested_blocks: HashSet::new(),
        }
    }

    /// Try to find a missing block this peer has and we don't
    pub fn pick_block(&self, peer_bitfield: &[bool]) -> Option<BlockInfo> {
        for (i, piece) in self.pieces.iter().enumerate() {
            if piece.is_none() && peer_bitfield.get(i).copied().unwrap_or(false) {
                // Find next block in this piece that we need
                if let Some(block_info) = self.find_next_block_in_piece(i) {
                    return Some(block_info);
                }
            }
        }
        None
    }

    /// Find the next block we need in a specific piece
    fn find_next_block_in_piece(&self, piece_index: usize) -> Option<BlockInfo> {
        let blocks_in_piece = self.get_blocks_in_piece(piece_index);

        for block_offset in (0..blocks_in_piece).map(|i| i * PIECE_BLOCK_SIZE) {
            let block_length = self.get_block_length(piece_index, block_offset);
            let block_info = BlockInfo {
                piece_index,
                offset: block_offset,
                length: block_length,
            };

            if !self.requested_blocks.contains(&block_info) {
                // Check if we already have this block
                if let Some(piece_blocks) = self.piece_blocks.get(&piece_index) {
                    let block_index = block_offset / PIECE_BLOCK_SIZE;
                    if block_index < piece_blocks.len() && piece_blocks[block_index].is_some() {
                        continue; // Already have this block
                    }
                }
                return Some(block_info);
            }
        }
        None
    }

    /// Get number of blocks in a piece
    fn get_blocks_in_piece(&self, piece_index: usize) -> usize {
        let piece_length = self.piece_length as usize;
        (piece_length + PIECE_BLOCK_SIZE - 1) / PIECE_BLOCK_SIZE
    }

    /// Get the length of a specific block
    fn get_block_length(&self, piece_index: usize, block_offset: usize) -> usize {
        let piece_length = self.piece_length as usize;
        let remaining = piece_length - block_offset;
        remaining.min(PIECE_BLOCK_SIZE)
    }

    /// Try to find a missing piece this peer has and we don't (legacy method)
    pub fn pick_piece(&self, peer_bitfield: &[bool]) -> Option<usize> {
        for (i, piece) in self.pieces.iter().enumerate() {
            if piece.is_none()
                && peer_bitfield.get(i).copied().unwrap_or(false)
                && !self.requested.contains(&i)
            {
                return Some(i);
            }
        }
        None
    }

    /// Mark that we are trying to download a block
    pub fn mark_block_requested(&mut self, block_info: BlockInfo) {
        self.requested_blocks.insert(block_info);
    }

    /// Mark that we are trying to download a piece (legacy method)
    pub fn mark_requested(&mut self, index: usize) {
        self.requested.insert(index);
    }

    pub fn unmark_piece(&mut self, index: usize) {
        self.requested.remove(&index);
    }

    /// Store a downloaded block and check if piece is complete
    pub fn complete_block(&mut self, block_info: BlockInfo, data: Vec<u8>) -> bool {
        // Remove from requested blocks
        self.requested_blocks.remove(&block_info);

        // Initialize piece blocks if needed
        if !self.piece_blocks.contains_key(&block_info.piece_index) {
            let blocks_in_piece = self.get_blocks_in_piece(block_info.piece_index);
            self.piece_blocks
                .insert(block_info.piece_index, vec![None; blocks_in_piece]);
        }

        // Store the block
        if let Some(piece_blocks) = self.piece_blocks.get_mut(&block_info.piece_index) {
            let block_index = block_info.offset / PIECE_BLOCK_SIZE;
            if block_index < piece_blocks.len() {
                piece_blocks[block_index] = Some(data);

                // Check if all blocks are complete
                if piece_blocks.iter().all(|block| block.is_some()) {
                    // Combine all blocks into complete piece
                    let mut complete_piece = Vec::new();
                    for block in piece_blocks.iter() {
                        if let Some(block_data) = block {
                            complete_piece.extend_from_slice(block_data);
                        }
                    }

                    // Store complete piece
                    self.pieces[block_info.piece_index] = Some(complete_piece);
                    self.requested.remove(&block_info.piece_index);
                    self.piece_blocks.remove(&block_info.piece_index);
                    return true; // Piece is complete
                }
            }
        }
        false // Piece not yet complete
    }

    /// Once we got the piece data, store it and stop tracking it (legacy method)
    pub fn complete_piece(&mut self, index: usize, data: Vec<u8>) {
        self.pieces[index] = Some(data);
        self.requested.remove(&index);
    }

    /// Has all pieces?
    pub fn is_complete(&self) -> bool {
        self.pieces.iter().all(|p| p.is_some())
    }

    /// How many pieces downloaded
    pub fn progress(&self) -> usize {
        self.pieces.iter().filter(|p| p.is_some()).count()
    }

    /// Get progress of a specific piece (percentage of blocks downloaded)
    pub fn piece_progress(&self, piece_index: usize) -> f64 {
        if let Some(piece_blocks) = self.piece_blocks.get(&piece_index) {
            let completed_blocks = piece_blocks.iter().filter(|block| block.is_some()).count();
            completed_blocks as f64 / piece_blocks.len() as f64
        } else if self.pieces[piece_index].is_some() {
            1.0 // Complete
        } else {
            0.0 // Not started
        }
    }

    /// Get total number of blocks across all pieces
    pub fn total_blocks(&self) -> usize {
        (0..self.total_pieces)
            .map(|i| self.get_blocks_in_piece(i))
            .sum()
    }

    /// Get number of completed blocks
    pub fn completed_blocks(&self) -> usize {
        let mut completed = 0;

        // Count completed pieces
        for (i, piece) in self.pieces.iter().enumerate() {
            if piece.is_some() {
                completed += self.get_blocks_in_piece(i);
            }
        }

        // Count partial pieces
        for (piece_index, blocks) in &self.piece_blocks {
            if self.pieces[*piece_index].is_none() {
                completed += blocks.iter().filter(|block| block.is_some()).count();
            }
        }

        completed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_info_creation() {
        let block = BlockInfo {
            piece_index: 5,
            offset: 16384,
            length: 16384,
        };
        assert_eq!(block.piece_index, 5);
        assert_eq!(block.offset, 16384);
        assert_eq!(block.length, 16384);
    }

    #[test]
    fn test_download_state_block_operations() {
        let mut state = DownloadState::new(10, 32768, [1u8; 20]);

        // Test get_blocks_in_piece
        assert_eq!(state.get_blocks_in_piece(0), 2); // 32768 / 16384 = 2 blocks

        // Test get_block_length
        assert_eq!(state.get_block_length(0, 0), 16384);
        assert_eq!(state.get_block_length(0, 16384), 16384);

        // Test total_blocks
        assert_eq!(state.total_blocks(), 20); // 10 pieces * 2 blocks each
    }

    #[test]
    fn test_pick_block() {
        let mut state = DownloadState::new(3, 32768, [1u8; 20]);
        let peer_bitfield = vec![true, false, true]; // Has pieces 0 and 2

        // Should pick first block of piece 0
        let block = state.pick_block(&peer_bitfield).unwrap();
        assert_eq!(block.piece_index, 0);
        assert_eq!(block.offset, 0);
        assert_eq!(block.length, 16384);

        // Mark it as requested
        state.mark_block_requested(block.clone());

        // Should pick second block of piece 0
        let block2 = state.pick_block(&peer_bitfield).unwrap();
        assert_eq!(block2.piece_index, 0);
        assert_eq!(block2.offset, 16384);
        assert_eq!(block2.length, 16384);
    }

    #[test]
    fn test_complete_block() {
        let mut state = DownloadState::new(2, 32768, [1u8; 20]);

        let block1 = BlockInfo {
            piece_index: 0,
            offset: 0,
            length: 16384,
        };

        let block2 = BlockInfo {
            piece_index: 0,
            offset: 16384,
            length: 16384,
        };

        // Complete first block - piece should not be complete yet
        let piece_complete = state.complete_block(block1, vec![1u8; 16384]);
        assert!(!piece_complete);
        assert_eq!(state.progress(), 0);

        // Complete second block - piece should be complete now
        let piece_complete = state.complete_block(block2, vec![2u8; 16384]);
        assert!(piece_complete);
        assert_eq!(state.progress(), 1);
    }

    #[test]
    fn test_piece_progress() {
        let mut state = DownloadState::new(2, 32768, [1u8; 20]);

        // Initially no progress
        assert_eq!(state.piece_progress(0), 0.0);

        // Complete one block
        let block1 = BlockInfo {
            piece_index: 0,
            offset: 0,
            length: 16384,
        };
        state.complete_block(block1, vec![1u8; 16384]);

        // Should be 50% complete
        assert_eq!(state.piece_progress(0), 0.5);

        // Complete second block
        let block2 = BlockInfo {
            piece_index: 0,
            offset: 16384,
            length: 16384,
        };
        state.complete_block(block2, vec![2u8; 16384]);

        // Should be 100% complete
        assert_eq!(state.piece_progress(0), 1.0);
    }

    #[test]
    fn test_completed_blocks_count() {
        let mut state = DownloadState::new(2, 32768, [1u8; 20]);

        assert_eq!(state.completed_blocks(), 0);

        // Complete one block
        let block1 = BlockInfo {
            piece_index: 0,
            offset: 0,
            length: 16384,
        };
        state.complete_block(block1, vec![1u8; 16384]);

        assert_eq!(state.completed_blocks(), 1);

        // Complete second block (completes piece)
        let block2 = BlockInfo {
            piece_index: 0,
            offset: 16384,
            length: 16384,
        };
        state.complete_block(block2, vec![2u8; 16384]);

        assert_eq!(state.completed_blocks(), 2);
    }

    #[test]
    fn test_block_request_tracking() {
        let mut state = DownloadState::new(2, 32768, [1u8; 20]);

        let block_info = BlockInfo {
            piece_index: 0,
            offset: 0,
            length: 16384,
        };

        // Mark as requested
        state.mark_block_requested(block_info.clone());
        assert!(state.requested_blocks.contains(&block_info));

        // Complete the block
        state.complete_block(block_info.clone(), vec![1u8; 16384]);
        assert!(!state.requested_blocks.contains(&block_info));
    }

    #[test]
    fn test_find_next_block_in_piece() {
        let mut state = DownloadState::new(2, 32768, [1u8; 20]);

        // Should find first block
        let block = state.find_next_block_in_piece(0).unwrap();
        assert_eq!(block.offset, 0);

        // Mark first block as requested
        state.mark_block_requested(block);

        // Should find second block
        let block2 = state.find_next_block_in_piece(0).unwrap();
        assert_eq!(block2.offset, 16384);

        // Complete first block
        let block1 = BlockInfo {
            piece_index: 0,
            offset: 0,
            length: 16384,
        };
        state.complete_block(block1, vec![1u8; 16384]);

        // Should still find second block (not yet requested)
        let block3 = state.find_next_block_in_piece(0).unwrap();
        assert_eq!(block3.offset, 16384);
    }

    #[test]
    fn test_legacy_methods_still_work() {
        let mut state = DownloadState::new(5, 16384, [1u8; 20]);
        let peer_bitfield = vec![true, false, true, false, true];

        // Legacy pick_piece should still work
        let piece = state.pick_piece(&peer_bitfield).unwrap();
        assert_eq!(piece, 0);

        // Legacy mark_requested should still work
        state.mark_requested(piece);
        assert!(state.requested.contains(&piece));

        // Legacy complete_piece should still work
        state.complete_piece(piece, vec![1u8; 16384]);
        assert_eq!(state.progress(), 1);
    }
}
