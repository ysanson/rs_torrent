use crate::peer::Peer;
use crate::peer::handshake::Handshake;
use crate::peer::message::{Bitfield, Message, MessageId};
use crate::peer::state::{BlockInfo, DownloadState};

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;

pub const PIECE_BLOCK_SIZE: usize = 16384; // 16KB blocks
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(45); // Increased for multi-peer scenarios
const MAX_PIPELINE_DEPTH: usize = 5; // Reduced to prevent peer overload
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30); // Reduced for faster recovery
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(120); // Keep-alive interval

/// Represents the state of a connection to a peer
#[derive(Debug, Clone)]
pub struct PeerConnection {
    pub peer: Peer,
    pub am_choking: bool,
    pub am_interested: bool,
    pub peer_choking: bool,
    pub peer_interested: bool,
    pub bitfield: Option<Bitfield>,
    pub pending_requests: HashMap<BlockInfo, std::time::Instant>, // Blocks we've requested with timestamps
    pub last_request_time: Option<std::time::Instant>,
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
            pending_requests: HashMap::new(),
            last_request_time: None,
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
}

/// Statistics for pipelining performance
#[derive(Debug, Clone, Default)]
pub struct PipelineStats {
    pub total_requests_sent: u64,
    pub total_responses_received: u64,
    pub total_timeouts: u64,
    pub max_pipeline_depth_seen: usize,
    pub average_response_time_ms: f64,
}

/// Main BitTorrent client for downloading from peers
pub struct BitTorrentClient {
    pub download_state: Arc<Mutex<DownloadState>>,
    pub connections: Arc<Mutex<HashMap<SocketAddr, PeerConnection>>>,
    pub pipeline_stats: Arc<Mutex<PipelineStats>>,
    pub peer_id: [u8; 20],
}

impl BitTorrentClient {
    pub fn new(download_state: DownloadState, peer_id: [u8; 20]) -> Self {
        Self {
            download_state: Arc::new(Mutex::new(download_state)),
            connections: Arc::new(Mutex::new(HashMap::new())),
            pipeline_stats: Arc::new(Mutex::new(PipelineStats::default())),
            peer_id,
        }
    }

    /// Connect to a peer and perform the handshake
    pub async fn connect_to_peer(
        &self,
        peer: &Peer,
    ) -> Result<TcpStream, Box<dyn std::error::Error>> {
        let addr = SocketAddr::from((peer.ip_addr, peer.port));
        let stream = timeout(CONNECTION_TIMEOUT, TcpStream::connect(addr)).await??;

        // Perform handshake
        let download_state = self.download_state.lock().await;
        let handshake = Handshake {
            infohash: download_state.info_hash,
            peer_id: self.peer_id,
        };
        drop(download_state);

        self.perform_handshake(stream, handshake).await
    }

    /// Perform the BitTorrent handshake with a peer
    async fn perform_handshake(
        &self,
        mut stream: TcpStream,
        handshake: Handshake,
    ) -> Result<TcpStream, Box<dyn std::error::Error>> {
        // Send our handshake
        let handshake_bytes = handshake.serialize();
        stream.write_all(&handshake_bytes).await?;

        // Receive peer's handshake
        let mut response = [0u8; 68];
        timeout(READ_TIMEOUT, stream.read_exact(&mut response)).await??;

        // Verify the handshake
        let peer_handshake =
            Handshake::deserialize(&response).ok_or("Invalid handshake response")?;

        if peer_handshake.infohash != handshake.infohash {
            return Err("Infohash mismatch".into());
        }

        println!("✅ Handshake successful with peer - infohash matches");
        Ok(stream)
    }

    /// Main peer worker that handles communication with a single peer
    pub async fn peer_worker(&self, peer: Peer) -> Result<(), Box<dyn std::error::Error>> {
        let mut stream = self.connect_to_peer(&peer).await?;
        let addr = SocketAddr::from((peer.ip_addr, peer.port));
        println!("🔗 Connected to peer {}", addr);

        // Initialize connection state
        {
            let mut connections = self.connections.lock().await;
            connections.insert(addr, PeerConnection::new(peer));
        }

        // Send interested message
        let interested_msg = Message {
            kind: MessageId::Interested,
            payload: vec![],
        };
        stream.write_all(&interested_msg.serialize()).await?;
        println!("📤 Sent 'interested' to peer {}", addr);

        // Send unchoke message to encourage peer to unchoke us (tit-for-tat)
        let unchoke_msg = Message {
            kind: MessageId::Unchoke,
            payload: vec![],
        };
        stream.write_all(&unchoke_msg.serialize()).await?;
        println!("📤 Sent 'unchoke' to peer {}", addr);

        // Update our state
        {
            let mut connections = self.connections.lock().await;
            if let Some(conn) = connections.get_mut(&addr) {
                conn.am_interested = true;
                conn.am_choking = false;
            }
        }

        // Wait a bit for peer to send initial messages (bitfield, etc.)
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Message handling loop
        loop {
            match self.handle_peer_messages(&mut stream, &addr).await {
                Ok(should_continue) => {
                    if !should_continue {
                        break;
                    }
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    if error_msg.contains("deadline has elapsed") {
                        println!(
                            "⏰ Read timeout for peer {}, attempting to recover...",
                            addr
                        );
                        // Don't break immediately on timeout, try to recover
                        continue;
                    } else {
                        eprintln!("Error handling peer {}: {}", addr, e);
                        break;
                    }
                }
            }

            // Handle request timeouts
            if let Err(e) = self.handle_request_timeouts(&addr).await {
                eprintln!("Error handling timeouts for peer {}: {}", addr, e);
            }

            // Try to download blocks if we can
            if let Err(e) = self.try_download_blocks(&mut stream, &addr).await {
                // Don't break on "not ready" errors, just continue
                let error_msg = e.to_string();
                if !error_msg.contains("not ready") && !error_msg.contains("timeout") {
                    eprintln!("Error downloading from peer {}: {}", addr, e);
                    break;
                } else if error_msg.contains("timeout") {
                    println!(
                        "⏰ Timeout in try_download_blocks for peer {}, continuing...",
                        addr
                    );
                }
            }

            // Print diagnostics every 60 seconds for debugging (reduced frequency)
            {
                let connections = self.connections.lock().await;
                if let Some(conn) = connections.get(&addr) {
                    if conn
                        .last_request_time
                        .map(|t| t.elapsed().as_secs() > 60)
                        .unwrap_or(true)
                    {
                        drop(connections);
                        self.print_peer_diagnostics().await;
                    }
                }
            }

            // Send keep-alive if no recent activity
            if let Err(e) = self.send_keep_alive_if_needed(&mut stream, &addr).await {
                eprintln!("Error sending keep-alive to peer {}: {}", addr, e);
                break;
            }

            // Check if download is complete
            {
                let download_state = self.download_state.lock().await;
                if download_state.is_complete() {
                    println!("Download complete!");
                    break;
                }
            }
        }

        // Clean up connection
        {
            let mut connections = self.connections.lock().await;
            connections.remove(&addr);
        }
        println!("🔌 Disconnected from peer {}", addr);

        Ok(())
    }

    /// Handle incoming messages from a peer
    async fn handle_peer_messages(
        &self,
        stream: &mut TcpStream,
        addr: &SocketAddr,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        // Read message length with adaptive timeout
        let mut len_buf = [0u8; 4];
        match timeout(
            Duration::from_millis(10000),
            stream.read_exact(&mut len_buf),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(_)) => {
                println!("🔌 Connection closed by peer {}", addr);
                return Ok(false); // Connection closed
            }
            Err(_) => {
                // Timeout - check if peer is still responsive
                println!("⏰ Read timeout for peer {}, continuing...", addr);
                return Ok(true); // Timeout, continue
            }
        }

        let msg_len = u32::from_be_bytes(len_buf) as usize;
        if msg_len == 0 {
            // Keep-alive message received
            println!("💓 Received keep-alive from peer {}", addr);
            return Ok(true);
        }

        // Read the full message with better error handling
        let mut msg_buf = vec![0u8; msg_len];
        match timeout(READ_TIMEOUT, stream.read_exact(&mut msg_buf)).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                println!(
                    "🔌 Connection error reading message from peer {}: {}",
                    addr, e
                );
                return Ok(false); // Connection closed
            }
            Err(_) => {
                println!("⏰ Timeout reading message from peer {}", addr);
                return Err("deadline has elapsed".into()); // Let caller handle timeout
            }
        }

        // Reconstruct the full message buffer
        let mut full_msg = Vec::with_capacity(4 + msg_len);
        full_msg.extend_from_slice(&len_buf);
        full_msg.extend_from_slice(&msg_buf);

        // Parse message
        let message = Message::deserialize(&full_msg).ok_or("Failed to deserialize message")?;

        self.process_message(message, addr).await?;
        Ok(true)
    }

    /// Process a received message from a peer
    async fn process_message(
        &self,
        message: Message,
        addr: &SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match message.kind {
            MessageId::Choke => {
                let mut connections = self.connections.lock().await;
                if let Some(conn) = connections.get_mut(addr) {
                    conn.peer_choking = true;
                }
                println!("Peer {} choked us", addr);
            }
            MessageId::Unchoke => {
                let mut connections = self.connections.lock().await;
                if let Some(conn) = connections.get_mut(addr) {
                    conn.peer_choking = false;
                }
                println!("✅ Peer {} unchoked us - can now download!", addr);
            }
            MessageId::Interested => {
                let mut connections = self.connections.lock().await;
                if let Some(conn) = connections.get_mut(addr) {
                    conn.peer_interested = true;
                }
            }
            MessageId::NotInterested => {
                let mut connections = self.connections.lock().await;
                if let Some(conn) = connections.get_mut(addr) {
                    conn.peer_interested = false;
                }
            }
            MessageId::Have => {
                if message.payload.len() == 4 {
                    let piece_index = u32::from_be_bytes([
                        message.payload[0],
                        message.payload[1],
                        message.payload[2],
                        message.payload[3],
                    ]) as usize;

                    let mut connections = self.connections.lock().await;
                    if let Some(conn) = connections.get_mut(addr) {
                        if let Some(ref mut bitfield) = conn.bitfield {
                            bitfield.set_piece(piece_index);
                        }
                    }
                    println!("Peer {} has piece {}", addr, piece_index);
                }
            }
            MessageId::Bitfield => {
                let bitfield = Bitfield::try_from(message)?;
                let piece_count = {
                    let download_state = self.download_state.lock().await;
                    download_state.total_pieces
                };
                let available_pieces = (0..piece_count).filter(|&i| bitfield.has_piece(i)).count();

                let mut connections = self.connections.lock().await;
                if let Some(conn) = connections.get_mut(addr) {
                    conn.bitfield = Some(bitfield);
                }
                println!(
                    "📋 Received bitfield from peer {} ({} pieces available)",
                    addr, available_pieces
                );
            }
            MessageId::Piece => {
                self.handle_piece_message(message, addr).await?;
            }
            MessageId::Request => {
                // We don't handle incoming requests in this simple client
            }
            MessageId::Cancel => {
                // We don't handle cancellations in this simple client
            }
        }
        Ok(())
    }

    /// Handle a piece message from a peer
    async fn handle_piece_message(
        &self,
        message: Message,
        addr: &SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if message.payload.len() < 8 {
            return Err("Invalid piece message".into());
        }

        let piece_index = u32::from_be_bytes([
            message.payload[0],
            message.payload[1],
            message.payload[2],
            message.payload[3],
        ]) as usize;

        let block_offset = u32::from_be_bytes([
            message.payload[4],
            message.payload[5],
            message.payload[6],
            message.payload[7],
        ]) as usize;

        let block_data = &message.payload[8..];

        println!(
            "Received piece {} block at offset {} ({} bytes) from {}",
            piece_index,
            block_offset,
            block_data.len(),
            addr
        );

        // Create block info for this received block
        let block_info = BlockInfo {
            piece_index,
            offset: block_offset,
            length: block_data.len(),
        };

        // Remove from pending requests and update stats
        {
            let mut connections = self.connections.lock().await;
            if let Some(conn) = connections.get_mut(addr) {
                if conn.remove_pending_request(&block_info) {
                    // Update response stats
                    let mut stats = self.pipeline_stats.lock().await;
                    stats.total_responses_received += 1;
                }
            }
        }

        // Store the block and check if piece is complete
        let piece_result = {
            let mut download_state = self.download_state.lock().await;
            download_state.complete_block(block_info, block_data.to_vec())
        };

        match piece_result {
            Ok(true) => {
                // Piece is complete and verified
                let download_state = self.download_state.lock().await;
                println!(
                    "✅ Piece {} verified and completed ({}/{})",
                    piece_index,
                    download_state.progress(),
                    download_state.total_pieces
                );
            }
            Ok(false) => {
                // Block stored but piece not yet complete
                let download_state = self.download_state.lock().await;
                let piece_progress = download_state.piece_progress(piece_index);
                println!(
                    "Piece {} progress: {:.1}% ({} blocks completed)",
                    piece_index,
                    piece_progress * 100.0,
                    download_state.completed_blocks()
                );
            }
            Err(e) => {
                // SHA1 verification failed
                println!("❌ {}", e);
                // Re-request all blocks for this piece since verification failed
                // The DownloadState has already cleaned up the failed piece
                return Err(e.into());
            }
        }

        Ok(())
    }

    /// Try to download blocks from a peer
    async fn try_download_blocks(
        &self,
        stream: &mut TcpStream,
        addr: &SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (can_download, peer_state) = {
            let connections = self.connections.lock().await;
            if let Some(conn) = connections.get(addr) {
                let can_download = conn.can_download();
                let state = format!(
                    "choking={}, interested={}, peer_choking={}, peer_interested={}, has_bitfield={}",
                    conn.am_choking,
                    conn.am_interested,
                    conn.peer_choking,
                    conn.peer_interested,
                    conn.bitfield.is_some()
                );
                (can_download, state)
            } else {
                (false, "no connection".to_string())
            }
        };

        if !can_download {
            // Only log occasionally to avoid spam
            let should_log = {
                let connections = self.connections.lock().await;
                connections
                    .get(addr)
                    .and_then(|conn| conn.last_request_time)
                    .map(|last| last.elapsed().as_secs() > 10)
                    .unwrap_or(true)
            };
            if should_log {
                println!("🚫 Peer {} not ready: {}", addr, peer_state);
            }
            return Ok(()); // Don't error, just skip this cycle
        }

        // Fill pipeline with requests up to MAX_PIPELINE_DEPTH
        loop {
            // Check pipeline capacity and find a block atomically
            let (block_to_download, can_pipeline) = {
                let mut download_state = self.download_state.lock().await;
                let connections = self.connections.lock().await;

                if let Some(conn) = connections.get(addr) {
                    let can_pipeline = conn.can_pipeline_request();
                    if can_pipeline {
                        if let Some(ref bitfield) = conn.bitfield {
                            // Convert bitfield to bool vector for compatibility
                            let peer_bitfield: Vec<bool> = (0..download_state.total_pieces)
                                .map(|i| bitfield.has_piece(i))
                                .collect();

                            // Atomically pick block and mark as requested to prevent race conditions
                            if let Some(block_info) = download_state.pick_block(&peer_bitfield) {
                                download_state.mark_block_requested(block_info.clone());
                                (Some(block_info), true)
                            } else {
                                (None, true)
                            }
                        } else {
                            (None, true)
                        }
                    } else {
                        (None, false)
                    }
                } else {
                    (None, false)
                }
            };

            if !can_pipeline {
                break; // Can't send more requests due to pipeline limit
            }

            if let Some(block_info) = block_to_download {
                // Add to peer's pending requests
                {
                    let mut connections = self.connections.lock().await;
                    if let Some(conn) = connections.get_mut(addr) {
                        conn.add_pending_request(block_info.clone());
                    }
                }

                // Send the request
                self.request_block(stream, block_info).await?;

                // Update pipeline stats
                {
                    let mut stats = self.pipeline_stats.lock().await;
                    stats.total_requests_sent += 1;

                    let connections = self.connections.lock().await;
                    if let Some(conn) = connections.get(addr) {
                        let current_depth = conn.pending_requests.len();
                        if current_depth > stats.max_pipeline_depth_seen {
                            stats.max_pipeline_depth_seen = current_depth;
                        }
                    }
                }
            } else {
                // No more blocks available
                break;
            }
        }

        Ok(())
    }

    /// Check if we can send more requests to this peer

    /// Request a block from a peer
    async fn request_block(
        &self,
        stream: &mut TcpStream,
        block_info: BlockInfo,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let request_msg = Message {
            kind: MessageId::Request,
            payload: {
                let mut payload = Vec::new();
                payload.extend_from_slice(&(block_info.piece_index as u32).to_be_bytes());
                payload.extend_from_slice(&(block_info.offset as u32).to_be_bytes());
                payload.extend_from_slice(&(block_info.length as u32).to_be_bytes());
                payload
            },
        };

        stream.write_all(&request_msg.serialize()).await?;

        // Get pipeline depth for logging
        let pipeline_depth = {
            let connections = self.connections.lock().await;
            connections
                .get(&SocketAddr::from((
                    stream.peer_addr()?.ip(),
                    stream.peer_addr()?.port(),
                )))
                .map(|conn| conn.pending_requests.len())
                .unwrap_or(0)
        };

        println!(
            "📤 Requested block: piece {}, offset {}, length {} from peer {} (pipeline: {})",
            block_info.piece_index,
            block_info.offset,
            block_info.length,
            stream.peer_addr()?,
            pipeline_depth
        );
        Ok(())
    }

    /// Handle request timeouts for a peer
    async fn handle_request_timeouts(
        &self,
        addr: &SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let timed_out_requests = {
            let mut connections = self.connections.lock().await;
            if let Some(conn) = connections.get_mut(addr) {
                conn.clear_timed_out_requests()
            } else {
                Vec::new()
            }
        };

        if !timed_out_requests.is_empty() {
            println!(
                "⏰ {} requests timed out for peer {}, removing from download state",
                timed_out_requests.len(),
                addr
            );

            // Update timeout stats
            {
                let mut stats = self.pipeline_stats.lock().await;
                stats.total_timeouts += timed_out_requests.len() as u64;
            }

            // Remove timed out requests from download state so they can be re-requested
            let mut download_state = self.download_state.lock().await;
            for block_info in timed_out_requests {
                download_state.requested_blocks.remove(&block_info);
            }
        }

        Ok(())
    }

    /// Send keep-alive message if needed
    async fn send_keep_alive_if_needed(
        &self,
        stream: &mut TcpStream,
        addr: &SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let should_send_keepalive = {
            let connections = self.connections.lock().await;
            if let Some(conn) = connections.get(addr) {
                if let Some(last_time) = conn.last_request_time {
                    last_time.elapsed() > KEEP_ALIVE_INTERVAL
                } else {
                    true // No previous activity, send keep-alive
                }
            } else {
                false
            }
        };

        if should_send_keepalive {
            // Send keep-alive (length = 0)
            let keep_alive = [0u8; 4];
            stream.write_all(&keep_alive).await?;
            println!("💓 Sent keep-alive to peer {}", addr);

            // Update last activity time
            let mut connections = self.connections.lock().await;
            if let Some(conn) = connections.get_mut(addr) {
                conn.last_request_time = Some(std::time::Instant::now());
            }
        }

        Ok(())
    }

    /// Start downloading from multiple peers concurrently
    pub async fn start_download(&self, peers: Vec<Peer>) -> Result<(), Box<dyn std::error::Error>> {
        let mut handles = Vec::new();

        for peer in peers {
            let client = self.clone();
            let handle = tokio::spawn(async move {
                if let Err(e) = client.peer_worker(peer).await {
                    eprintln!("Peer worker error: {}", e);
                }
            });
            handles.push(handle);
        }

        // Wait for all workers to complete
        for handle in handles {
            handle.await?;
        }

        Ok(())
    }

    /// Get download progress
    pub async fn get_progress(&self) -> (usize, usize) {
        let download_state = self.download_state.lock().await;
        (download_state.progress(), download_state.total_pieces)
    }

    /// Get detailed block-level progress
    pub async fn get_block_progress(&self) -> (usize, usize) {
        let download_state = self.download_state.lock().await;
        (
            download_state.completed_blocks(),
            download_state.total_blocks(),
        )
    }

    /// Check if download is complete
    pub async fn is_complete(&self) -> bool {
        let download_state = self.download_state.lock().await;
        download_state.is_complete()
    }

    /// Get pipelining statistics
    pub async fn get_pipeline_stats(&self) -> PipelineStats {
        let stats = self.pipeline_stats.lock().await;
        stats.clone()
    }

    /// Get detailed peer connection information
    pub async fn get_peer_info(&self) -> Vec<(SocketAddr, usize, bool)> {
        let connections = self.connections.lock().await;
        connections
            .iter()
            .map(|(addr, conn)| (*addr, conn.pending_requests.len(), conn.can_download()))
            .collect()
    }

    /// Print diagnostic information about all peer connections
    pub async fn print_peer_diagnostics(&self) {
        let connections = self.connections.lock().await;
        println!("\n=== PEER CONNECTION DIAGNOSTICS ===");

        if connections.is_empty() {
            println!("No active peer connections");
            return;
        }

        for (addr, conn) in connections.iter() {
            let bitfield_pieces = if let Some(ref bf) = conn.bitfield {
                let download_state = self.download_state.lock().await;
                (0..download_state.total_pieces)
                    .filter(|&i| bf.has_piece(i))
                    .count()
            } else {
                0
            };

            println!("Peer {}:", addr);
            println!(
                "  State: choking_us={}, we_interested={}, we_choking={}, they_interested={}",
                conn.peer_choking, conn.am_interested, conn.am_choking, conn.peer_interested
            );
            println!("  Bitfield: has {} pieces available", bitfield_pieces);
            println!(
                "  Pipeline: {} pending requests",
                conn.pending_requests.len()
            );
            println!("  Can download: {}", conn.can_download());
            println!(
                "  Last activity: {:?}",
                conn.last_request_time.map(|t| t.elapsed())
            );
            println!();
        }
        println!("=== END DIAGNOSTICS ===\n");
    }

    pub async fn write_to_file(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.download_state.lock().await.write_to_file(path).await
    }
}

impl Clone for BitTorrentClient {
    fn clone(&self) -> Self {
        Self {
            download_state: Arc::clone(&self.download_state),
            connections: Arc::clone(&self.connections),
            pipeline_stats: Arc::clone(&self.pipeline_stats),
            peer_id: self.peer_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
