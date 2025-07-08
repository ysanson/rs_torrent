//! BitTorrent Client Example
//!
//! This example demonstrates how to create a BitTorrent client that can download
//! pieces from peers using the handshake, message, and state management components.

use rs_torrent::peer::Peer;
use rs_torrent::peer::client::BitTorrentClient;
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
    println!("Total size: {} bytes\n", torrent.total_size);

    // Step 2: Create download state
    let download_state =
        DownloadState::new(torrent.pieces.len(), torrent.piece_length, torrent.infohash);

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
            eprintln!("Download error: {}", e);
        }
    });

    // Step 6: Monitor progress
    loop {
        let (downloaded, total) = client.get_progress().await;
        let progress = (downloaded as f64 / total as f64) * 100.0;

        println!(
            "Progress: {}/{} pieces ({:.1}%)",
            downloaded, total, progress
        );

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
    let download_state =
        DownloadState::new(torrent.pieces.len(), torrent.piece_length, torrent.infohash);

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
=== HOW TO BUILD A BITTORRENT CLIENT STEP BY STEP ===

1. **Understanding the Components**:
   - `Handshake`: Establishes connection with peer using BitTorrent protocol
   - `Message`: Handles peer communication (choke, unchoke, interested, have, bitfield, request, piece)
   - `DownloadState`: Manages which pieces we have and need
   - `BitTorrentClient`: Orchestrates the entire download process

2. **Connection Flow**:
   ```
   Connect to Peer → Handshake → Exchange Bitfields → Request Pieces → Download Data
   ```

3. **Message Exchange Protocol**:
   - Send "interested" to let peer know we want data
   - Wait for "unchoke" message from peer
   - Send "request" messages for specific pieces
   - Receive "piece" messages with actual data
   - Handle "have" messages when peer gets new pieces

4. **Key Implementation Steps**:

   a) **Handshake Phase**:
      - Create handshake with torrent's infohash and your peer_id
      - Send handshake bytes to peer
      - Receive and validate peer's handshake response

   b) **Message Handling**:
      - Parse incoming messages using Message::deserialize()
      - Handle different message types appropriately
      - Update peer state (choking, interested, bitfield)

   c) **Piece Management**:
      - Use DownloadState to track which pieces we need
      - Request pieces from peers that have them and aren't choking us
      - Handle piece messages and store downloaded data

   d) **Concurrency**:
      - Use async/await for non-blocking I/O
      - Spawn separate tasks for each peer connection
      - Use Arc<Mutex<>> for shared state between tasks

5. **Error Handling**:
   - Handle connection timeouts
   - Validate message formats
   - Deal with peers that disconnect
   - Retry failed downloads

6. **Optimization Opportunities**:
   - Pipeline multiple requests to same peer
   - Split large pieces into smaller blocks (16KB)
   - Implement endgame mode for final pieces
   - Add piece verification with SHA1 hashes
   - Implement tit-for-tat strategy for uploading

7. **Real-world Considerations**:
   - Rate limiting to avoid overwhelming peers
   - Proper peer discovery via DHT
   - Support for multiple trackers
   - Handling partial files and resume capability
   - Implementing the full BitTorrent protocol extensions

8. **Testing**:
   - Start with simple test peers
   - Use small torrent files initially
   - Monitor network traffic and debug messages
   - Test with different peer behaviors

This example provides a foundation that you can extend with additional features
like multi-tracker support, DHT peer discovery, piece verification, and more
sophisticated peer management strategies.
*/
