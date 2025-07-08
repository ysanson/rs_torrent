# Block-Based BitTorrent Download Implementation

## Overview

This document explains the block-based downloading approach implemented in the BitTorrent client, which divides pieces into smaller 16KB blocks for more efficient and granular downloading.

## Key Concepts

### 1. Block Structure

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockInfo {
    pub piece_index: usize,  // Which piece this block belongs to
    pub offset: usize,       // Byte offset within the piece
    pub length: usize,       // Block size (usually 16KB)
}
```

### 2. Constants

```rust
const PIECE_BLOCK_SIZE: usize = 16384; // 16KB blocks
```

## Architecture

### State Management

The `DownloadState` struct has been enhanced to handle blocks:

```rust
pub struct DownloadState {
    pub total_pieces: usize,
    pub piece_length: u32,
    pub info_hash: [u8; 20],
    pub pieces: Vec<Option<Vec<u8>>>,                    // Complete pieces
    pub requested: HashSet<usize>,                       // Legacy piece tracking
    pub piece_blocks: HashMap<usize, Vec<Option<Vec<u8>>>>, // Blocks within pieces
    pub requested_blocks: HashSet<BlockInfo>,            // Block-level requests
}
```

### Key Methods

#### Block Discovery
```rust
pub fn pick_block(&self, peer_bitfield: &[bool]) -> Option<BlockInfo>
```
- Finds the next block to download from available peers
- Prioritizes completing partial pieces
- Avoids requesting already-requested blocks

#### Block Completion
```rust
pub fn complete_block(&mut self, block_info: BlockInfo, data: Vec<u8>) -> bool
```
- Stores a downloaded block
- Returns `true` if the piece is now complete
- Automatically assembles complete pieces from blocks

#### Progress Tracking
```rust
pub fn piece_progress(&self, piece_index: usize) -> f64
pub fn completed_blocks(&self) -> usize
pub fn total_blocks(&self) -> usize
```

## Download Flow

### 1. Connection Setup
```
Client → Peer: TCP Connect
Client → Peer: Handshake
Client ← Peer: Handshake Response
Client → Peer: Interested
Client ← Peer: Bitfield
Client ← Peer: Unchoke
```

### 2. Block Request Process
```
1. Client calls pick_block() to find next needed block
2. Client calls mark_block_requested() to track the request
3. Client sends Request message with (piece_index, offset, length)
4. Peer responds with Piece message containing block data
5. Client calls complete_block() to store the data
6. If piece is complete, it's moved to the pieces array
```

### 3. Request Message Format
```
Request Message:
- Message ID: 6 (Request)
- Payload: [piece_index(4 bytes)][offset(4 bytes)][length(4 bytes)]
```

### 4. Piece Message Format
```
Piece Message:
- Message ID: 7 (Piece)
- Payload: [piece_index(4 bytes)][offset(4 bytes)][block_data(variable)]
```

## Implementation Details

### Block Size Calculation

For a piece of length `piece_length`:
- Number of blocks = `(piece_length + PIECE_BLOCK_SIZE - 1) / PIECE_BLOCK_SIZE`
- Last block length = `piece_length % PIECE_BLOCK_SIZE` or `PIECE_BLOCK_SIZE`

### Example: 32KB Piece
- Block 0: offset=0, length=16384
- Block 1: offset=16384, length=16384
- Total blocks: 2

### Example: 20KB Piece
- Block 0: offset=0, length=16384
- Block 1: offset=16384, length=4096
- Total blocks: 2

## Benefits of Block-Based Approach

### 1. **Improved Parallelism**
- Multiple blocks can be requested from the same peer simultaneously
- Reduces idle time waiting for large piece downloads

### 2. **Better Progress Tracking**
- Granular progress reporting at block level
- Users see continuous progress instead of large jumps

### 3. **Fault Tolerance**
- If connection fails, only lose progress on current block
- Can resume from last completed block

### 4. **Efficient Bandwidth Usage**
- Smaller request sizes reduce memory usage
- Better network utilization with pipelining

### 5. **Endgame Optimization**
- Final blocks can be requested from multiple peers
- Reduces time to complete final pieces

## Usage Example

```rust
// Create download state
let download_state = DownloadState::new(total_pieces, piece_length, info_hash);

// Create client
let client = BitTorrentClient::new(download_state, peer_id);

// Monitor progress
let (completed_blocks, total_blocks) = client.get_block_progress().await;
let progress = (completed_blocks as f64 / total_blocks as f64) * 100.0;
println!("Block progress: {:.1}%", progress);
```

## Testing

The implementation includes comprehensive tests:

```bash
cargo test peer::state::tests
```

Test coverage includes:
- Block info creation and manipulation
- Block discovery and selection
- Block completion and piece assembly
- Progress tracking at block level
- Request tracking and cleanup
- Legacy method compatibility

## Performance Considerations

### Memory Usage
- Each block requires temporary storage until piece completion
- Memory scales with number of partial pieces
- Completed pieces are stored as single Vec<u8>

### Network Efficiency
- Optimal block size balances memory usage and network overhead
- 16KB blocks are standard BitTorrent practice
- Pipelining multiple requests improves throughput

### CPU Usage
- Block assembly requires copying data
- HashMap lookups for block tracking
- Minimal compared to network I/O

## Future Enhancements

### 1. **Request Pipelining**
- Send multiple block requests without waiting for responses
- Configurable pipeline depth per peer

### 2. **Smart Block Selection**
- Prioritize rarest blocks first
- Consider peer upload speeds in selection

### 3. **Endgame Mode**
- Request final blocks from multiple peers
- Cancel duplicate requests when received

### 4. **Block Caching**
- Cache frequently requested blocks
- Reduce redundant downloads

### 5. **Bandwidth Management**
- Rate limiting per peer
- Global bandwidth controls

## Compatibility

The block-based implementation maintains backward compatibility:
- Legacy `pick_piece()` and `complete_piece()` methods still work
- Existing code can be migrated incrementally
- Block-level methods provide enhanced functionality

## Conclusion

The block-based downloading approach significantly improves the efficiency and user experience of BitTorrent downloads. By breaking pieces into smaller, manageable blocks, the client can:

- Provide better progress feedback
- Utilize network resources more efficiently
- Recover from failures more gracefully
- Support advanced features like endgame mode

This implementation provides a solid foundation for a production-quality BitTorrent client while maintaining simplicity and testability.