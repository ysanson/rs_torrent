use crate::peer::Peer;
use crate::peer::handshake::Handshake;
use crate::peer::message::build_piece_response;
use crate::peer::message::{
    Bitfield, ExtendedMessage, ExtendedMessageId, Message, MessageId, PieceRequest,
};
use crate::peer::state::{BlockInfo, DownloadState};

use log::{debug, error};
use rustc_hash::FxHashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, mpsc};
use tokio::time::timeout;

pub const PIECE_BLOCK_SIZE: usize = 16384;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PIPELINE_DEPTH: usize = 10;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(120);
const BATCH_SIZE: usize = 16;

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

/// Indexed connections for more efficient peer lookups
#[derive(Debug)]
pub struct IndexedConnections {
    connections: Vec<Option<PeerConnection>>,
    addr_to_index: FxHashMap<SocketAddr, usize>,
    free_indices: Vec<usize>,
}

impl Default for IndexedConnections {
    fn default() -> Self {
        Self::new()
    }
}

impl IndexedConnections {
    pub fn new() -> Self {
        Self {
            connections: Vec::new(),
            addr_to_index: FxHashMap::default(),
            free_indices: Vec::new(),
        }
    }

    pub fn insert(&mut self, addr: SocketAddr, conn: PeerConnection) {
        if let Some(index) = self.free_indices.pop() {
            self.connections[index] = Some(conn);
            self.addr_to_index.insert(addr, index);
        } else {
            let index = self.connections.len();
            self.connections.push(Some(conn));
            self.addr_to_index.insert(addr, index);
        }
    }

    pub fn get(&self, addr: &SocketAddr) -> Option<&PeerConnection> {
        self.addr_to_index
            .get(addr)
            .and_then(|&idx| self.connections.get(idx))
            .and_then(|opt| opt.as_ref())
    }

    pub fn get_mut(&mut self, addr: &SocketAddr) -> Option<&mut PeerConnection> {
        self.addr_to_index
            .get(addr)
            .and_then(|&idx| self.connections.get_mut(idx))
            .and_then(|opt| opt.as_mut())
    }

    pub fn remove(&mut self, addr: &SocketAddr) -> Option<PeerConnection> {
        if let Some(&index) = self.addr_to_index.get(addr) {
            self.addr_to_index.remove(addr);
            let removed = self.connections[index].take();
            self.free_indices.push(index);
            removed
        } else {
            None
        }
    }

    pub fn contains_key(&self, addr: &SocketAddr) -> bool {
        self.addr_to_index.contains_key(addr)
    }

    pub fn is_empty(&self) -> bool {
        self.addr_to_index.is_empty()
    }

    pub fn len(&self) -> usize {
        self.addr_to_index.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&SocketAddr, &PeerConnection)> {
        self.addr_to_index
            .iter()
            .filter_map(move |(addr, &idx)| self.connections[idx].as_ref().map(|conn| (addr, conn)))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&SocketAddr, &mut PeerConnection)> {
        // This is a bit more complex due to borrowing rules, but more efficient than HashMap
        let addr_to_index = &self.addr_to_index;
        self.connections
            .iter_mut()
            .enumerate()
            .filter_map(|(idx, conn_opt)| {
                if let Some(conn) = conn_opt.as_mut() {
                    // Find the address for this index
                    addr_to_index
                        .iter()
                        .find(|&(_, &i)| i == idx)
                        .map(|(addr, _)| (addr, conn))
                } else {
                    None
                }
            })
    }
}

/// Main BitTorrent client for downloading from peers
pub struct BitTorrentClient {
    pub download_state: Arc<Mutex<DownloadState>>,
    pub connections: Arc<Mutex<IndexedConnections>>,
    pub pipeline_stats: Arc<Mutex<PipelineStats>>,
    pub peer_id: [u8; 20],
}

impl BitTorrentClient {
    pub fn new(download_state: DownloadState, peer_id: [u8; 20]) -> Self {
        Self {
            download_state: Arc::new(Mutex::new(download_state)),
            connections: Arc::new(Mutex::new(IndexedConnections::new())),
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

        debug!("✅ Handshake successful with peer - infohash matches");
        Ok(stream)
    }

    /// Main peer worker that handles communication with a single peer
    pub async fn peer_worker(&self, peer: Peer) -> Result<(), Box<dyn std::error::Error>> {
        let mut stream = self.connect_to_peer(&peer).await?;
        let addr = SocketAddr::from((peer.ip_addr, peer.port));
        debug!("🔗 Connected to peer {addr}");

        // Initialize connection state
        {
            let mut connections = self.connections.lock().await;
            connections.insert(addr, PeerConnection::new(peer));
        }

        // Send interested + unchoke before splitting the stream
        let interested_msg = Message {
            kind: MessageId::Interested,
            payload: vec![],
        };
        stream.write_all(&interested_msg.serialize()).await?;
        debug!("📤 Sent 'interested' to peer {addr}");

        let unchoke_msg = Message {
            kind: MessageId::Unchoke,
            payload: vec![],
        };
        stream.write_all(&unchoke_msg.serialize()).await?;
        debug!("📤 Sent 'unchoke' to peer {addr}");

        {
            let mut connections = self.connections.lock().await;
            if let Some(conn) = connections.get_mut(&addr) {
                conn.am_interested = true;
                conn.am_choking = false;
            }
        }

        // Give the peer a moment to send its initial bitfield/unchoke
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Per-peer cache: owns its own buffers and a piece offset derived from the
        // peer address so different peers start scanning from different pieces.
        let total_pieces = self.download_state.lock().await.total_pieces;
        let mut perf_cache = PerformanceCache::new_for_peer(total_pieces, &addr);

        // Split the stream: a background task reads continuously; the main loop writes.
        let (mut read_half, mut write_half) = stream.into_split();
        let (msg_tx, mut msg_rx) = mpsc::channel::<Message>(32);

        let reader_task = tokio::spawn(async move {
            while let Ok(Some(msg)) = read_raw_message(&mut read_half).await {
                if msg_tx.send(msg).await.is_err() {
                    break;
                }
            }
        });

        // Main loop: process messages when they arrive OR tick every 100 ms to
        // refill the request pipeline even when the peer is quiet.
        loop {
            tokio::select! {
                biased;
                msg_opt = msg_rx.recv() => {
                    match msg_opt {
                        Some(msg) => {
                            if let Err(e) = self.process_message(msg, &addr, &mut write_half).await {
                                debug!("Error processing message from {addr}: {e}");
                                break;
                            }
                        }
                        None => {
                            debug!("🔌 Connection closed by peer {addr}");
                            break;
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }

            if let Err(e) = self.handle_request_timeouts(&addr).await {
                debug!("Error handling timeouts for peer {addr}: {e}");
            }

            if let Err(e) = self
                .try_download_blocks(&mut write_half, &addr, &mut perf_cache)
                .await
            {
                debug!("Error downloading from peer {addr}: {e}");
                break;
            }

            // Print diagnostics every 60 seconds
            {
                let connections = self.connections.lock().await;
                if let Some(conn) = connections.get(&addr)
                    && conn
                        .last_request_time
                        .map(|t| t.elapsed().as_secs() > 60)
                        .unwrap_or(true)
                {
                    drop(connections);
                    self.print_peer_diagnostics().await;
                }
            }

            if let Err(e) = self.send_keep_alive_if_needed(&mut write_half, &addr).await {
                debug!("Error sending keep-alive to peer {addr}: {e}");
                break;
            }

            if self.download_state.lock().await.is_complete() {
                debug!("Download complete!");
                break;
            }
        }

        reader_task.abort();

        // Clean up connection and any pending requests
        {
            let mut download_state = self.download_state.lock().await;
            let mut connections = self.connections.lock().await;
            if let Some(conn) = connections.get(&addr) {
                for block_info in conn.pending_requests.keys() {
                    download_state.requested_blocks.remove(block_info);
                }
            }
            connections.remove(&addr);
        }
        debug!("🔌 Disconnected from peer {addr}");

        Ok(())
    }

    /// Process a received message from a peer
    async fn process_message(
        &self,
        message: Message,
        addr: &SocketAddr,
        write_half: &mut OwnedWriteHalf,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match message.kind {
            MessageId::Choke => {
                let mut connections = self.connections.lock().await;
                if let Some(conn) = connections.get_mut(addr) {
                    conn.peer_choking = true;
                }
                debug!("Peer {addr} choked us");
            }
            MessageId::Unchoke => {
                let mut connections = self.connections.lock().await;
                if let Some(conn) = connections.get_mut(addr) {
                    conn.peer_choking = false;
                }
                debug!("✅ Peer {addr} unchoked us - can now download!");
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
                    if let Some(conn) = connections.get_mut(addr)
                        && let Some(ref mut bitfield) = conn.bitfield
                    {
                        bitfield.set_piece(piece_index);
                    }
                    debug!("📢 Peer {addr} has piece {piece_index}");
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
                debug!(
                    "📋 Received bitfield from peer {addr} ({available_pieces} pieces available)"
                );
            }
            MessageId::Piece => {
                self.handle_piece_message(message, addr).await?;
            }
            MessageId::Request => {
                self.handle_piece_request(message, addr, write_half).await?;
            }
            MessageId::Cancel => {
                // We don't handle cancellations in this simple client
            }
            MessageId::Extended => {
                // Not yet implemented.
            }
        }
        Ok(())
    }

    async fn handle_piece_request(
        &self,
        message: Message,
        addr: &SocketAddr,
        write_half: &mut OwnedWriteHalf,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Ok(piece_request) = PieceRequest::try_from(&message) {
            if piece_request.length > 16 * 1024 {
                return Ok(());
            }
            let connections = self.connections.lock().await;
            let am_choking = connections.get(addr).is_none_or(|c| c.am_choking);
            drop(connections);
            if am_choking {
                return Ok(());
            }

            let mut download_state = self.download_state.lock().await;
            if let Some(piece) = download_state.get_piece_block(
                piece_request.piece_index as usize,
                piece_request.begin as usize,
                piece_request.length as usize,
            ) {
                let message =
                    build_piece_response(piece_request.piece_index, piece_request.begin, &piece);
                write_half.write_all(&message.serialize()).await?;
                download_state.record_upload(piece_request.length as u64);
            }
            drop(download_state);
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

        debug!(
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

        // Remove from pending requests and update stats first
        // CONSISTENT LOCK ORDER: download_state first, then connections
        let request_was_pending = {
            let _download_state = self.download_state.lock().await;
            let mut connections = self.connections.lock().await;

            if let Some(conn) = connections.get_mut(addr) {
                conn.remove_pending_request(&block_info)
            } else {
                false
            }
        };

        // Update stats separately to avoid holding multiple locks
        if request_was_pending {
            let mut stats = self.pipeline_stats.lock().await;
            stats.total_responses_received += 1;
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
                debug!(
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
                debug!(
                    "Piece {} progress: {:.1}% ({} blocks completed)",
                    piece_index,
                    piece_progress * 100.0,
                    download_state.completed_blocks()
                );
            }
            Err(e) => {
                // SHA1 verification failed
                error!("❌ {e}");
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
        stream: &mut OwnedWriteHalf,
        addr: &SocketAddr,
        perf_cache: &mut PerformanceCache,
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
                debug!("🚫 Peer {addr} not ready: {peer_state}");
            }
            return Ok(()); // Don't error, just skip this cycle
        }

        // Batch multiple block requests to reduce lock contention
        self.try_download_blocks_batched(stream, addr, perf_cache)
            .await?;

        Ok(())
    }

    /// Request a block from a peer
    async fn request_block(
        &self,
        stream: &mut OwnedWriteHalf,
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

        debug!(
            "📤 Requested block: piece {}, offset {}, length {} from peer {} (pipeline: {})",
            block_info.piece_index,
            block_info.offset,
            block_info.length,
            stream.peer_addr()?,
            pipeline_depth
        );
        Ok(())
    }

    /// Optimized batched block downloading with reduced lock contention
    async fn try_download_blocks_batched(
        &self,
        stream: &mut OwnedWriteHalf,
        addr: &SocketAddr,
        perf_cache: &mut PerformanceCache,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Get multiple blocks to request in a single lock acquisition
        let blocks_to_request = {
            let mut download_state = self.download_state.lock().await;
            let connections = self.connections.lock().await;

            perf_cache.clear();

            if let Some(conn) = connections.get(addr)
                && let Some(ref bitfield) = conn.bitfield
            {
                perf_cache
                    .bitfield_buffer
                    .extend((0..download_state.total_pieces).map(|i| bitfield.has_piece(i)));

                let current_pending = conn.pending_requests.len();
                let available_slots = MAX_PIPELINE_DEPTH.saturating_sub(current_pending);
                let max_blocks = std::cmp::min(BATCH_SIZE, available_slots);

                let piece_offset = perf_cache.piece_offset;
                for _ in 0..max_blocks {
                    if let Some(block_info) =
                        download_state.pick_block(&perf_cache.bitfield_buffer, piece_offset)
                    {
                        download_state.mark_block_requested(block_info.clone());
                        perf_cache.block_batch.push(block_info);
                    } else {
                        break;
                    }
                }
            }

            perf_cache.block_batch.clone()
        };

        if blocks_to_request.is_empty() {
            return Ok(());
        }

        // Add all blocks to pending requests in a single lock acquisition
        {
            let mut connections = self.connections.lock().await;
            if let Some(conn) = connections.get_mut(addr) {
                for block_info in &blocks_to_request {
                    conn.add_pending_request(block_info.clone());
                }
            }
        }

        // Send all requests without holding locks
        let mut requests_sent = 0;
        let mut failed_blocks = Vec::new();
        for block_info in &blocks_to_request {
            if self.request_block(stream, block_info.clone()).await.is_ok() {
                requests_sent += 1;
            } else {
                // Collect remaining blocks that weren't sent
                failed_blocks.extend(blocks_to_request[requests_sent..].iter().cloned());
                break; // Stop on first error
            }
        }

        // Clean up failed requests from both pending_requests and requested_blocks
        if !failed_blocks.is_empty() {
            let mut download_state = self.download_state.lock().await;
            let mut connections = self.connections.lock().await;

            if let Some(conn) = connections.get_mut(addr) {
                for block_info in &failed_blocks {
                    conn.pending_requests.remove(block_info);
                    download_state.requested_blocks.remove(block_info);
                }
            }

            debug!(
                "⚠️ Failed to send {} block requests to peer {}",
                failed_blocks.len(),
                addr
            );
        }

        // Update stats once for the entire batch
        if requests_sent > 0 {
            let current_depth = {
                let connections = self.connections.lock().await;
                connections
                    .get(addr)
                    .map(|conn| conn.pending_requests.len())
                    .unwrap_or(0)
            };

            let mut stats = self.pipeline_stats.lock().await;
            stats.total_requests_sent += requests_sent as u64;
            if current_depth > stats.max_pipeline_depth_seen {
                stats.max_pipeline_depth_seen = current_depth;
            }
        }

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
            debug!(
                "⏰ {} requests timed out for peer {}, removing from download state",
                timed_out_requests.len(),
                addr
            );

            // CONSISTENT LOCK ORDER: download_state first, then stats separately
            // Remove timed out requests from download state so they can be re-requested
            {
                let mut download_state = self.download_state.lock().await;
                for block_info in &timed_out_requests {
                    download_state.requested_blocks.remove(block_info);
                }
            }

            // Update timeout stats (separate lock, no ordering conflict)
            {
                let mut stats = self.pipeline_stats.lock().await;
                stats.total_timeouts += timed_out_requests.len() as u64;
            }
        }

        Ok(())
    }

    /// Send keep-alive message if needed
    async fn send_keep_alive_if_needed(
        &self,
        stream: &mut OwnedWriteHalf,
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
            debug!("💓 Sent keep-alive to peer {addr}");

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
        self.connect_to_new_peers(peers).await;

        // Keep running until download is complete
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if self.is_complete().await {
                break;
            }
        }

        Ok(())
    }

    /// Connect to new peers without waiting for completion (for reannouncement)
    pub async fn connect_to_new_peers(&self, peers: Vec<Peer>) {
        let mut new_peers = Vec::new();

        // Filter out peers we're already connected to
        {
            let connections = self.connections.lock().await;
            for peer in peers {
                let addr = std::net::SocketAddr::from((peer.ip_addr, peer.port));
                if !connections.contains_key(&addr) {
                    new_peers.push(peer);
                }
            }
        }

        if new_peers.is_empty() {
            return;
        }

        debug!("🔗 Connecting to {} new peers", new_peers.len());

        for peer in new_peers {
            let client = self.clone();
            tokio::spawn(async move {
                if let Err(e) = client.peer_worker(peer).await {
                    debug!("Peer worker error: {e}");
                }
            });
        }
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

    pub async fn get_uploaded_bytes(&self) -> u64 {
        let download_state = self.download_state.lock().await;
        download_state.uploaded
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
        // CONSISTENT LOCK ORDER: download_state first, then connections
        let (connection_info, requested_blocks_count, total_pending) = {
            let download_state = self.download_state.lock().await;
            let connections = self.connections.lock().await;

            if connections.is_empty() {
                debug!("\n=== PEER CONNECTION DIAGNOSTICS ===");
                debug!("No active peer connections");
                debug!(
                    "Requested blocks in download state: {}",
                    download_state.requested_blocks.len()
                );
                return;
            }

            let total_pieces = download_state.total_pieces;
            let requested_blocks_count = download_state.requested_blocks.len();
            let total_pending: usize = connections
                .iter()
                .map(|(_, conn)| conn.pending_requests.len())
                .sum();

            let info = connections
                .iter()
                .map(|(addr, conn)| {
                    let bitfield_pieces = if let Some(ref bf) = conn.bitfield {
                        (0..total_pieces).filter(|&i| bf.has_piece(i)).count()
                    } else {
                        0
                    };
                    (*addr, conn.clone(), bitfield_pieces)
                })
                .collect::<Vec<_>>();

            (info, requested_blocks_count, total_pending)
        };

        debug!("\n=== PEER CONNECTION DIAGNOSTICS ===");
        debug!(
            "Total requested blocks in download state: {}",
            requested_blocks_count
        );
        debug!("Total pending requests across all peers: {}", total_pending);

        for (addr, conn, bitfield_pieces) in connection_info {
            debug!(
                "Peer {}:
                    State: choking_us={}, we_interested={}, we_choking={}, they_interested={}
                    Bitfield: has {} pieces available
                    Pipeline: {} pending requests
                    Can download: {}
                    Last activity: {:?}
                    ",
                addr,
                conn.peer_choking,
                conn.am_interested,
                conn.am_choking,
                conn.peer_interested,
                bitfield_pieces,
                conn.pending_requests.len(),
                conn.can_download(),
                conn.last_request_time.map(|t| t.elapsed())
            );
        }
        debug!("=== END DIAGNOSTICS ===\n");
    }

    pub async fn write_to_file(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.download_state.lock().await.write_to_file(path).await
    }

    pub fn create_extension_handshake() -> Message {
        let mut payload = Vec::new();
        payload.push(ExtendedMessageId::Handshake as u8);
        payload.extend_from_slice(b"d1:md11:ut_metadatai1ee1:me"); // Example extension handshake

        Message {
            kind: MessageId::Extended,
            payload,
        }
    }

    pub fn create_metadata_request(piece: u32) -> Message {
        let mut payload = Vec::new();
        payload.push(ExtendedMessageId::Metadata as u8);
        payload.extend_from_slice(&piece.to_be_bytes());

        Message {
            kind: MessageId::Extended,
            payload,
        }
    }

    pub fn parse_metadata_response(payload: &[u8]) -> Option<Vec<u8>> {
        if payload.is_empty() {
            return None;
        }

        let extended_message = ExtendedMessage::deserialize(payload)?;
        if extended_message.kind != ExtendedMessageId::Metadata {
            return None;
        }

        Some(extended_message.payload)
    }
}

/// Reads a single BitTorrent message from the read half of a peer connection.
/// Keep-alive frames (length == 0) are consumed transparently.
/// Returns `Ok(None)` on a clean EOF.
async fn read_raw_message(
    stream: &mut OwnedReadHalf,
) -> Result<Option<Message>, Box<dyn std::error::Error + Send + Sync>> {
    loop {
        let mut len_buf = [0u8; 4];
        match stream.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let msg_len = u32::from_be_bytes(len_buf) as usize;
        if msg_len == 0 {
            // keep-alive — skip and read the next frame
            continue;
        }

        let mut msg_buf = vec![0u8; msg_len];
        match stream.read_exact(&mut msg_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let mut full_msg = Vec::with_capacity(4 + msg_len);
        full_msg.extend_from_slice(&len_buf);
        full_msg.extend_from_slice(&msg_buf);

        let message = Message::deserialize(&full_msg).ok_or("Failed to deserialize message")?;
        return Ok(Some(message));
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
