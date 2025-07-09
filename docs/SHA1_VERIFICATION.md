# SHA1 Verification in BitTorrent Client

## Overview

This document describes the SHA1 verification feature implemented in the BitTorrent client. SHA1 verification ensures data integrity by validating that downloaded pieces match their expected cryptographic hashes as specified in the torrent file.

## Key Features

- **Automatic Verification**: Every completed piece is automatically verified against its SHA1 hash
- **Block-Level Assembly**: Blocks are assembled into complete pieces before verification
- **Failure Recovery**: Failed verification discards corrupted data and allows re-downloading
- **Edge Case Support**: Proper handling of last pieces and blocks with non-standard sizes

## Implementation Details

### DownloadState Enhancement

The `DownloadState` struct now includes SHA1 verification capabilities:

```rust
pub struct DownloadState {
    pub total_pieces: usize,
    pub piece_length: u32,
    pub info_hash: [u8; 20],
    pub pieces: Vec<Option<Vec<u8>>>,
    pub requested: HashSet<usize>,
    pub piece_blocks: HashMap<usize, Vec<Option<Vec<u8>>>>,
    pub requested_blocks: HashSet<BlockInfo>,
    pub pieces_hash: Vec<[u8; 20]>,   // SHA1 hashes for each piece
    pub total_size: u64,              // Total size for last piece calculation
}
```

### Constructor

```rust
pub fn new(
    total_pieces: usize,
    piece_length: u32,
    info_hash: [u8; 20],
    pieces_hash: Vec<[u8; 20]>,
    total_size: u64,
) -> Self
```

### Verification Process

The verification happens in the `complete_block` method:

```rust
pub fn complete_block(&mut self, block_info: BlockInfo, data: Vec<u8>) -> Result<bool, String>
```

1. **Block Storage**: Store the received block data
2. **Completion Check**: Check if all blocks for the piece are complete
3. **Assembly**: Combine all blocks into a complete piece
4. **SHA1 Calculation**: Compute SHA1 hash of the assembled piece
5. **Verification**: Compare computed hash with expected hash
6. **Action**: Store verified piece or discard on failure

## Verification Flow

```
Block Received → Store Block → All Blocks Complete? → Assemble Piece → Calculate SHA1 → Verify Hash
                                      ↓                                                    ↓
                                   Continue                                           Pass/Fail
                                                                                         ↓
                                                                              Store Piece/Discard
```

## Error Handling

### Verification Failure

When SHA1 verification fails:
- All blocks for the piece are discarded
- The piece is marked as not requested
- An error message is returned with details
- The client can re-request the piece from other peers

### Error Message Format

```
"SHA1 verification failed for piece {index}: expected {expected_hash:?}, got {computed_hash:?}"
```

## Edge Cases Handled

### Last Piece Size

The last piece in a torrent may be smaller than the standard piece length:

```rust
fn get_actual_piece_length(&self, piece_index: usize) -> usize {
    if piece_index == self.total_pieces - 1 {
        // Last piece - calculate remaining bytes
        let bytes_in_completed_pieces = piece_index as u64 * self.piece_length as u64;
        (self.total_size - bytes_in_completed_pieces) as usize
    } else {
        self.piece_length as usize
    }
}
```

### Last Block Size

The last block in any piece may be smaller than the standard block size:

```rust
fn get_block_length(&self, piece_index: usize, block_offset: usize) -> usize {
    let actual_piece_length = self.get_actual_piece_length(piece_index);
    let remaining = actual_piece_length.saturating_sub(block_offset);
    remaining.min(PIECE_BLOCK_SIZE)
}
```

## Client Integration

The BitTorrent client handles verification results:

```rust
match piece_result {
    Ok(true) => {
        // Piece verified successfully
        println!("✅ Piece {} verified and completed", piece_index);
    }
    Ok(false) => {
        // Block stored but piece not complete
        println!("Piece {} progress: {:.1}%", piece_index, progress);
    }
    Err(e) => {
        // Verification failed
        println!("❌ {}", e);
        return Err(e.into());
    }
}
```

## Testing

### Test Coverage

The implementation includes comprehensive tests:

1. **Basic Verification**: Test successful SHA1 verification
2. **Failure Handling**: Test verification failure scenarios
3. **Multi-Block Pieces**: Test assembly of multiple blocks
4. **Last Piece Handling**: Test smaller last pieces
5. **Last Block Handling**: Test smaller last blocks
6. **Single Block Pieces**: Test pieces smaller than block size

### Example Test

```rust
#[test]
fn test_sha1_verification_success() {
    let test_data = vec![0x42u8; 16384];
    let expected_hash: [u8; 20] = Sha1::digest(&test_data).into();
    
    let mut state = DownloadState::new(1, 16384, [1u8; 20], vec![expected_hash], 16384);
    
    let block_info = BlockInfo {
        piece_index: 0,
        offset: 0,
        length: 16384,
    };
    
    let result = state.complete_block(block_info, test_data);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), true);
}
```

## Performance Considerations

### SHA1 Computation

- SHA1 is computed only when a piece is complete
- Uses the `sha1` crate for efficient computation
- Minimal CPU overhead compared to network I/O

### Memory Usage

- Blocks are stored temporarily until piece completion
- Memory usage scales with number of partial pieces
- Complete pieces are stored as single Vec<u8>

### Network Efficiency

- Failed verification triggers re-download from different peers
- Reduces overall network usage by ensuring data integrity
- Prevents corruption from propagating

## Benefits

1. **Data Integrity**: Ensures downloaded data matches expected content
2. **Corruption Detection**: Identifies network or peer corruption issues
3. **Automatic Recovery**: Seamlessly re-downloads corrupted pieces
4. **Standard Compliance**: Follows BitTorrent protocol requirements
5. **User Confidence**: Provides cryptographic guarantee of data correctness

## Future Enhancements

1. **Parallel Verification**: Verify multiple pieces concurrently
2. **Verification Caching**: Cache verification results to avoid re-computation
3. **Peer Reputation**: Track peers that send corrupted data
4. **Progress Indication**: Show verification progress for large pieces
5. **Partial Verification**: Verify blocks individually where possible

## Conclusion

The SHA1 verification feature provides essential data integrity guarantees while maintaining efficient block-based downloading. It properly handles edge cases, provides clear error reporting, and integrates seamlessly with the existing client architecture.