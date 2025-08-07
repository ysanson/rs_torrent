use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

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
    pub pieces_hash: Vec<[u8; 20]>,   // SHA1 hashes for each piece
    pub total_size: u64,              // Total size of all data
}

impl DownloadState {
    pub fn new(
        total_pieces: usize,
        piece_length: u32,
        info_hash: [u8; 20],
        pieces_hash: Vec<[u8; 20]>,
        total_size: u64,
    ) -> Self {
        Self {
            total_pieces,
            piece_length,
            info_hash,
            pieces: vec![None; total_pieces],
            requested: HashSet::new(),
            piece_blocks: HashMap::new(),
            requested_blocks: HashSet::new(),
            pieces_hash,
            total_size,
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
        let actual_piece_length = self.get_actual_piece_length(piece_index);
        actual_piece_length.div_ceil(PIECE_BLOCK_SIZE)
    }

    /// Get the length of a specific block
    fn get_block_length(&self, piece_index: usize, block_offset: usize) -> usize {
        let actual_piece_length = self.get_actual_piece_length(piece_index);
        let remaining = actual_piece_length.saturating_sub(block_offset);
        remaining.min(PIECE_BLOCK_SIZE)
    }

    /// Get the actual length of a piece (last piece may be shorter)
    fn get_actual_piece_length(&self, piece_index: usize) -> usize {
        if piece_index == self.total_pieces - 1 {
            // Last piece - calculate remaining bytes
            let bytes_in_completed_pieces = piece_index as u64 * self.piece_length as u64;
            (self.total_size - bytes_in_completed_pieces) as usize
        } else {
            self.piece_length as usize
        }
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
    pub fn complete_block(&mut self, block_info: BlockInfo, data: Vec<u8>) -> Result<bool, String> {
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
                    for block in piece_blocks.iter().flatten() {
                        complete_piece.extend_from_slice(block);
                    }

                    // Verify SHA1 hash
                    let computed_hash: [u8; 20] = Sha1::digest(&complete_piece).into();
                    let expected_hash = self.pieces_hash[block_info.piece_index];

                    if computed_hash == expected_hash {
                        // Store complete piece
                        self.pieces[block_info.piece_index] = Some(complete_piece);
                        self.requested.remove(&block_info.piece_index);
                        self.piece_blocks.remove(&block_info.piece_index);
                        return Ok(true); // Piece is complete and verified
                    } else {
                        // Hash verification failed - discard piece and all blocks
                        self.piece_blocks.remove(&block_info.piece_index);
                        self.requested.remove(&block_info.piece_index);
                        return Err(format!(
                            "SHA1 verification failed for piece {}: expected {:?}, got {:?}",
                            block_info.piece_index, expected_hash, computed_hash
                        ));
                    }
                }
            }
        }
        Ok(false) // Piece not yet complete
    }

    /// Once we got the piece data, store it and stop tracking it (legacy method)
    pub fn complete_piece(&mut self, index: usize, data: Vec<u8>) {
        self.pieces[index] = Some(data);
        self.requested.remove(&index);
        self.piece_blocks.remove(&index);
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
        } else if self
            .pieces
            .get(piece_index)
            .is_some_and(|piece| piece.is_some())
        {
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
            if self.pieces.get(*piece_index).is_some_and(|p| p.is_none()) {
                completed += blocks.iter().filter(|block| block.is_some()).count();
            }
        }

        completed
    }

    pub async fn write_to_file(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut file = File::create(path).await?;
        file.set_len(self.total_size).await?;
        for (i, piece) in self.pieces.iter().enumerate() {
            match piece {
                Some(data) => file.write_all(data).await?,
                None => return Err(format!("Piece {i} is missing").into()),
            }
        }

        Ok(())
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
        let state = DownloadState::new(10, 32768, [1u8; 20], vec![[0u8; 20]; 10], 327680);

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
        let mut state = DownloadState::new(3, 32768, [1u8; 20], vec![[0u8; 20]; 3], 98304);
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
        let mut state = DownloadState::new(2, 32768, [1u8; 20], vec![[0u8; 20]; 2], 65536);

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
        let piece_complete = state.complete_block(block1, vec![1u8; 16384]).unwrap();
        assert!(!piece_complete);
        assert_eq!(state.progress(), 0);

        // Complete second block - piece should be complete now (will fail SHA1 but test structure)
        let result = state.complete_block(block2, vec![2u8; 16384]);
        assert!(result.is_err()); // Should fail SHA1 verification
        assert_eq!(state.progress(), 0); // No progress since verification failed
    }

    #[test]
    fn test_piece_progress() {
        let mut state = DownloadState::new(2, 32768, [1u8; 20], vec![[0u8; 20]; 2], 65536);

        // Initially no progress
        assert_eq!(state.piece_progress(0), 0.0);

        // Complete one block
        let block1 = BlockInfo {
            piece_index: 0,
            offset: 0,
            length: 16384,
        };
        state.complete_block(block1, vec![1u8; 16384]).unwrap();

        // Should be 50% complete
        assert_eq!(state.piece_progress(0), 0.5);

        // Complete second block
        let block2 = BlockInfo {
            piece_index: 0,
            offset: 16384,
            length: 16384,
        };
        // This will fail SHA1 verification and reset progress
        let _ = state.complete_block(block2, vec![2u8; 16384]);

        // Should be 0% complete since verification failed
        assert_eq!(state.piece_progress(0), 0.0);
    }

    #[test]
    fn test_completed_blocks_count() {
        let mut state = DownloadState::new(2, 32768, [1u8; 20], vec![[0u8; 20]; 2], 65536);

        assert_eq!(state.completed_blocks(), 0);

        // Complete one block
        let block1 = BlockInfo {
            piece_index: 0,
            offset: 0,
            length: 16384,
        };
        state.complete_block(block1, vec![1u8; 16384]).unwrap();

        assert_eq!(state.completed_blocks(), 1);

        // Complete second block (will fail verification)
        let block2 = BlockInfo {
            piece_index: 0,
            offset: 16384,
            length: 16384,
        };
        let _ = state.complete_block(block2, vec![2u8; 16384]);

        assert_eq!(state.completed_blocks(), 0); // Reset due to failed verification
    }

    #[test]
    fn test_block_request_tracking() {
        let mut state = DownloadState::new(2, 32768, [1u8; 20], vec![[0u8; 20]; 2], 65536);

        let block_info = BlockInfo {
            piece_index: 0,
            offset: 0,
            length: 16384,
        };

        // Mark as requested
        state.mark_block_requested(block_info.clone());
        assert!(state.requested_blocks.contains(&block_info));

        // Complete the block
        let _ = state.complete_block(block_info.clone(), vec![1u8; 16384]);
        assert!(!state.requested_blocks.contains(&block_info));
    }

    #[test]
    fn test_find_next_block_in_piece() {
        let mut state = DownloadState::new(2, 32768, [1u8; 20], vec![[0u8; 20]; 2], 65536);

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
        let _ = state.complete_block(block1, vec![1u8; 16384]);

        // Should still find second block (not yet requested)
        let block3 = state.find_next_block_in_piece(0).unwrap();
        assert_eq!(block3.offset, 16384);
    }

    #[test]
    fn test_legacy_methods_still_work() {
        let mut state = DownloadState::new(5, 16384, [1u8; 20], vec![[0u8; 20]; 5], 81920);
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

    #[test]
    fn test_sha1_verification_success() {
        use sha1::{Digest, Sha1};

        // Create test data
        let test_data = vec![0x42u8; 16384]; // Single block piece
        let expected_hash: [u8; 20] = Sha1::digest(&test_data).into();

        let mut state = DownloadState::new(1, 16384, [1u8; 20], vec![expected_hash], 16384);

        let block_info = BlockInfo {
            piece_index: 0,
            offset: 0,
            length: 16384,
        };

        // Complete the block with correct data
        let result = state.complete_block(block_info, test_data);
        assert!(result.is_ok());
        assert!(result.unwrap()); // Piece should be complete and verified
        assert_eq!(state.progress(), 1);
    }

    #[test]
    fn test_sha1_verification_failure() {
        // Create test data with wrong hash
        let test_data = vec![0x42u8; 16384];
        let wrong_hash = [0x99u8; 20]; // Incorrect hash

        let mut state = DownloadState::new(1, 16384, [1u8; 20], vec![wrong_hash], 16384);

        let block_info = BlockInfo {
            piece_index: 0,
            offset: 0,
            length: 16384,
        };

        // Complete the block with data that doesn't match hash
        let result = state.complete_block(block_info, test_data);
        assert!(result.is_err());
        assert_eq!(state.progress(), 0); // No progress since verification failed

        // Verify error message contains expected information
        let error_msg = result.unwrap_err();
        assert!(error_msg.contains("SHA1 verification failed"));
        assert!(error_msg.contains("piece 0"));
    }

    #[test]
    fn test_sha1_verification_multi_block_piece() {
        use sha1::{Digest, Sha1};

        // Create a piece with 2 blocks
        let block1_data = vec![0x11u8; 16384];
        let block2_data = vec![0x22u8; 16384];
        let mut complete_piece = Vec::new();
        complete_piece.extend_from_slice(&block1_data);
        complete_piece.extend_from_slice(&block2_data);

        let expected_hash: [u8; 20] = Sha1::digest(&complete_piece).into();
        let mut state = DownloadState::new(1, 32768, [1u8; 20], vec![expected_hash], 32768);

        // Complete first block
        let block1_info = BlockInfo {
            piece_index: 0,
            offset: 0,
            length: 16384,
        };
        let result = state.complete_block(block1_info, block1_data);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Piece not complete yet
        assert_eq!(state.progress(), 0);

        // Complete second block
        let block2_info = BlockInfo {
            piece_index: 0,
            offset: 16384,
            length: 16384,
        };
        let result = state.complete_block(block2_info, block2_data);
        assert!(result.is_ok());
        assert!(result.unwrap()); // Piece should be complete and verified
        assert_eq!(state.progress(), 1);
    }

    #[test]
    fn test_last_piece_smaller_than_normal() {
        // Create a torrent where the last piece is smaller than piece_length
        // Total size: 50000 bytes, piece_length: 32768 bytes
        // Piece 0: 32768 bytes (2 blocks)
        // Piece 1: 17232 bytes (2 blocks: 16384 + 848)
        let piece_length = 32768;
        let total_size = 50000;
        let total_pieces = 2;

        let state = DownloadState::new(
            total_pieces,
            piece_length,
            [1u8; 20],
            vec![[0u8; 20]; total_pieces],
            total_size,
        );

        // Test normal piece (piece 0)
        assert_eq!(state.get_blocks_in_piece(0), 2); // 32768 / 16384 = 2 blocks
        assert_eq!(state.get_block_length(0, 0), 16384); // First block
        assert_eq!(state.get_block_length(0, 16384), 16384); // Second block

        // Test last piece (piece 1) - should be 17232 bytes
        assert_eq!(state.get_blocks_in_piece(1), 2); // 17232 / 16384 = 1.05 -> 2 blocks
        assert_eq!(state.get_block_length(1, 0), 16384); // First block
        assert_eq!(state.get_block_length(1, 16384), 848); // Second block (17232 - 16384 = 848)

        // Test actual piece length calculation
        assert_eq!(state.get_actual_piece_length(0), 32768);
        assert_eq!(state.get_actual_piece_length(1), 17232);
    }

    #[test]
    fn test_last_block_in_last_piece() {
        use sha1::{Digest, Sha1};

        // Create test data for a small last piece
        let block1_data = vec![0x11u8; 16384]; // Full block
        let block2_data = vec![0x22u8; 848]; // Partial block (last block)
        let mut complete_piece = Vec::new();
        complete_piece.extend_from_slice(&block1_data);
        complete_piece.extend_from_slice(&block2_data);

        let expected_hash: [u8; 20] = Sha1::digest(&complete_piece).into();
        let mut state = DownloadState::new(
            2,
            32768,
            [1u8; 20],
            vec![[0u8; 20], expected_hash],
            50000, // Total size that makes last piece smaller
        );

        // Complete first block of last piece
        let block1_info = BlockInfo {
            piece_index: 1,
            offset: 0,
            length: 16384,
        };
        let result = state.complete_block(block1_info, block1_data);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Piece not complete yet

        // Complete second block (partial) of last piece
        let block2_info = BlockInfo {
            piece_index: 1,
            offset: 16384,
            length: 848,
        };
        let result = state.complete_block(block2_info, block2_data);
        assert!(result.is_ok());
        assert!(result.unwrap()); // Piece should be complete and verified
    }

    #[test]
    fn test_single_block_last_piece() {
        use sha1::{Digest, Sha1};

        // Create a torrent where the last piece is smaller than one block
        let test_data = vec![0x42u8; 8192]; // 8KB - smaller than block size
        let expected_hash: [u8; 20] = Sha1::digest(&test_data).into();

        let mut state = DownloadState::new(
            2,
            16384,
            [1u8; 20],
            vec![[0u8; 20], expected_hash],
            24576, // 16384 + 8192
        );

        // Last piece should have only 1 block
        assert_eq!(state.get_blocks_in_piece(1), 1);
        assert_eq!(state.get_block_length(1, 0), 8192);
        assert_eq!(state.get_actual_piece_length(1), 8192);

        // Complete the single block
        let block_info = BlockInfo {
            piece_index: 1,
            offset: 0,
            length: 8192,
        };
        let result = state.complete_block(block_info, test_data);
        assert!(result.is_ok());
        assert!(result.unwrap()); // Piece should be complete and verified
    }
}
