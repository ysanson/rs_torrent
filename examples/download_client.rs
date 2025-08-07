//! BitTorrent Client Example
//!
//! This example demonstrates how to create a BitTorrent client that can download
//! pieces from peers using block-based downloading with 16KB blocks.

use rs_torrent::peer::Peer;
use rs_torrent::peer::client::{BitTorrentClient, PIECE_BLOCK_SIZE};
use rs_torrent::peer::state::DownloadState;
use rs_torrent::torrent::parse_torrent_file;
use std::net::Ipv4Addr;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== BitTorrent Client Example ===\n");

    // Step 1: Parse a torrent file (or create test data)
    let torrent = create_test_torrent();
    println!("Torrent loaded: {}", torrent.name);
    println!("Total pieces: {}", torrent.pieces.len());
    println!("Piece length: {} bytes", torrent.piece_length);
    println!("Block size: {PIECE_BLOCK_SIZE} bytes");
    println!("Total size: {} bytes\n", torrent.total_size);

    // Step 2: Create download state
    let download_state = DownloadState::new(
        torrent.pieces.len(),
        torrent.piece_length,
        torrent.infohash,
        torrent.pieces,
        torrent.total_size as u64,
    );

    // Step 3: Create client with a unique peer ID
    let peer_id = generate_peer_id();
    let client = BitTorrentClient::new(download_state, peer_id);

    // Step 4: Get peers from tracker (simulated here)
    let peers = get_test_peers();
    println!("Found {} peers:", peers.len());
    for peer in &peers {
        println!("  - {}:{}", peer.ip_addr, peer.port);
    }
    println!();

    // Step 5: Start the download process
    println!("Starting download...\n");

    // Start download in a separate task
    let client_clone = client.clone();
    let download_handle = tokio::spawn(async move {
        if let Err(e) = client_clone.start_download(peers).await {
            eprintln!("Download error: {e}");
        }
    });

    // Step 6: Monitor progress
    let mut diagnostic_counter = 0;
    loop {
        let (downloaded_pieces, total_pieces) = client.get_progress().await;
        let (downloaded_blocks, total_blocks) = client.get_block_progress().await;
        let pipeline_stats = client.get_pipeline_stats().await;
        let peer_info = client.get_peer_info().await;

        let piece_progress = (downloaded_pieces as f64 / total_pieces as f64) * 100.0;
        let block_progress = (downloaded_blocks as f64 / total_blocks as f64) * 100.0;

        println!(
            "Progress: {downloaded_pieces}/{total_pieces} pieces ({piece_progress:.1}%) | {downloaded_blocks}/{total_blocks} blocks ({block_progress:.1}%)"
        );

        // Show pipelining statistics
        println!(
            "Pipeline: {} requests sent, {} responses, {} timeouts, max depth: {}",
            pipeline_stats.total_requests_sent,
            pipeline_stats.total_responses_received,
            pipeline_stats.total_timeouts,
            pipeline_stats.max_pipeline_depth_seen
        );

        // Show active peer connections
        let active_peers = peer_info
            .iter()
            .filter(|(_, _, can_download)| *can_download)
            .count();
        let total_pending: usize = peer_info.iter().map(|(_, pending, _)| pending).sum();
        let connected_peers = peer_info.len();
        println!(
            "Peers: {connected_peers} connected, {active_peers} active, {total_pending} total pending requests"
        );

        // Show detailed diagnostics every 5 cycles (10 seconds)
        diagnostic_counter += 1;
        if diagnostic_counter >= 5 {
            client.print_peer_diagnostics().await;
            diagnostic_counter = 0;
        }

        if client.is_complete().await {
            println!("\n✅ Download complete!");
            break;
        }

        sleep(Duration::from_secs(2)).await;
    }

    // Wait for download task to complete
    download_handle.await?;

    Ok(())
}

/// Generate a unique peer ID for this client
fn generate_peer_id() -> [u8; 20] {
    let mut peer_id = [0u8; 20];
    let client_id = b"RS0001"; // Client identifier
    peer_id[..6].copy_from_slice(client_id);

    // Fill rest with random-ish data (in real implementation, use proper random)
    for i in 6..20 {
        peer_id[i] = (i * 17 + 42) as u8;
    }

    peer_id
}

/// Create a test torrent for demonstration
fn create_test_torrent() -> rs_torrent::Torrent {
    rs_torrent::Torrent {
        announce: "http://tracker.example.com:8080/announce".to_string(),
        creation_date: 1640995200, // 2022-01-01
        length: 1024 * 1024,       // 1MB
        piece_length: 16384,       // 16KB pieces
        total_size: 1024 * 1024,
        name: "example_file.txt".to_string(),
        pieces: vec![[0u8; 20]; 64], // 64 pieces for 1MB file
        infohash: [
            0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC,
            0xDE, 0xF0, 0x12, 0x34, 0x56, 0x78,
        ],
    }
}

/// Create test peers for demonstration
fn get_test_peers() -> Vec<Peer> {
    vec![
        Peer {
            ip_addr: Ipv4Addr::new(192, 168, 1, 100),
            port: 6881,
        },
        Peer {
            ip_addr: Ipv4Addr::new(10, 0, 0, 50),
            port: 6882,
        },
        Peer {
            ip_addr: Ipv4Addr::new(172, 16, 0, 25),
            port: 6883,
        },
    ]
}

/// Example of how to use the client with a real torrent file
#[allow(dead_code)]
async fn download_from_torrent_file(file_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Step 1: Parse the torrent file
    let torrent = parse_torrent_file(file_path)?;

    // Step 2: Create download state
    let download_state = DownloadState::new(
        torrent.pieces.len(),
        torrent.piece_length,
        torrent.infohash,
        torrent.pieces.clone(),
        torrent.total_size as u64,
    );

    // Step 3: Create client
    let peer_id = generate_peer_id();
    let client = BitTorrentClient::new(download_state, peer_id);

    // Step 4: Get peers from tracker
    let peers = rs_torrent::tracker::announce_to_tracker(&torrent, 6881).await?;
    println!("Found {} peers from tracker", peers.1.len());

    // Step 5: Start download
    client.start_download(peers.1).await?;

    Ok(())
}

/*
=== HOW TO BUILD A BITTORRENT CLIENT WITH REQUEST PIPELINING ===

1. **Understanding the Components**:
   - `Handshake`: Establishes connection with peer using BitTorrent protocol
   - `Message`: Handles peer communication (choke, unchoke, interested, have, bitfield, request, piece)
   - `DownloadState`: Manages which pieces and blocks we have and need
   - `BitTorrentClient`: Orchestrates the entire download process with pipelining
   - `BlockInfo`: Represents a 16KB block within a piece
   - `PeerConnection`: Tracks pending requests and pipeline state per peer

2. **Connection Flow**:
   ```
   Connect to Peer → Handshake → Exchange Bitfields → Pipeline Block Requests → Download Data
   ```

3. **Request Pipelining**:
   - Send multiple block requests without waiting for responses
   - Configurable pipeline depth (default: 10 requests per peer)
   - Automatic timeout handling for stalled requests
   - Improved bandwidth utilization and reduced latency

4. **Block-Based Downloading**:
   - Each piece is split into 16KB blocks (PIECE_BLOCK_SIZE)
   - Blocks are downloaded independently and can be pipelined
   - Piece is complete when all blocks are received
   - More efficient than downloading entire pieces at once

5. **Message Exchange Protocol**:
   - Send "interested" to let peer know we want data
   - Wait for "unchoke" message from peer
   - Send multiple "request" messages for specific blocks (pipelined)
   - Receive "piece" messages with actual block data
   - Handle "have" messages when peer gets new pieces
   - Track pending requests and handle timeouts

6. **Key Implementation Steps**:

   a) **Handshake Phase**:
      - Create handshake with torrent's infohash and your peer_id
      - Send handshake bytes to peer
      - Receive and validate peer's handshake response

   b) **Message Handling**:
      - Parse incoming messages using Message::deserialize()
      - Handle different message types appropriately
      - Update peer state (choking, interested, bitfield)

   c) **Pipelined Block Management**:
      - Use DownloadState to track which blocks we need
      - Send multiple block requests to fill pipeline (up to MAX_PIPELINE_DEPTH)
      - Track pending requests per peer with timestamps
      - Handle timeouts and re-request failed blocks
      - Handle piece messages and store downloaded block data
      - Combine blocks when all blocks of a piece are complete

   d) **Concurrency and Pipelining**:
      - Use async/await for non-blocking I/O
      - Spawn separate tasks for each peer connection
      - Use Arc<Mutex<>> for shared state between tasks
      - Pipeline multiple block requests to same peer
      - Monitor pipeline depth and adjust dynamically
      - Handle request timeouts and retries

7. **Pipeline Monitoring and Statistics**:
   - Track requests sent, responses received, and timeouts
   - Monitor maximum pipeline depth achieved
   - Show active peer connections and pending requests
   - Measure average response times

8. **Block-Level Progress Tracking**:
   - Track individual block completion
   - Show both piece-level and block-level progress
   - Handle partial piece downloads efficiently

9. **Error Handling**:
   - Handle connection timeouts
   - Validate message formats
   - Deal with peers that disconnect
   - Handle request timeouts and re-request blocks
   - Retry failed block downloads from different peers

10. **Optimization Features Implemented**:
    - Request pipelining with configurable depth
    - Automatic timeout detection and recovery
    - SHA1 piece verification
    - Smart block selection algorithms
    - Pipeline statistics and monitoring

11. **Future Optimizations**:
    - Dynamic pipeline depth adjustment based on peer performance
    - Implement endgame mode for final blocks
    - Implement tit-for-tat strategy for uploading
    - Adaptive request timeout based on peer latency

12. **Real-world Considerations**:
    - Rate limiting to avoid overwhelming peers
    - Proper peer discovery via DHT
    - Support for multiple trackers
    - Handling partial files and resume capability
    - Implementing the full BitTorrent protocol extensions
    - Pipeline backpressure and flow control

13. **Testing**:
    - Start with simple test peers
    - Use small torrent files initially
    - Monitor network traffic and debug messages
    - Test with different peer behaviors
    - Verify block assembly into complete pieces
    - Test pipeline behavior under various network conditions
    - Verify timeout handling and recovery

This pipelined, block-based approach provides significantly better performance
through parallelism, reduced latency, and more granular progress tracking
compared to downloading entire pieces sequentially.
*/
