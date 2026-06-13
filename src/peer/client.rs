use crate::peer::handshake::Handshake;
use crate::peer::message::{Bitfield, Message, MessageId, PieceRequest};
use crate::peer::message::{ExtendedMessageId, build_piece_response};
use crate::peer::state::{BlockInfo, DownloadState};
use crate::peer::{Peer, metadata};

use super::connection::{MAX_PIPELINE_DEPTH, PeerConnection};
use super::pool::IndexedConnections;
use super::stats::{BATCH_SIZE, PerformanceCache, PipelineStats};

use log::{debug, error};
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
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(120);

/// Main BitTorrent client for downloading from peers
pub struct BitTorrentClient {
    pub download_state: Arc<Mutex<DownloadState>>,
    pub connections: Arc<Mutex<IndexedConnections>>,
    pub pipeline_stats: Arc<Mutex<PipelineStats>>,
    pub peer_id: [u8; 20],
    pub info_dict: Option<Arc<Vec<u8>>>,
}

impl BitTorrentClient {
    pub fn new(
        download_state: DownloadState,
        peer_id: [u8; 20],
        info_dict: Option<Arc<Vec<u8>>>,
    ) -> Self {
        Self {
            download_state: Arc::new(Mutex::new(download_state)),
            connections: Arc::new(Mutex::new(IndexedConnections::new())),
            pipeline_stats: Arc::new(Mutex::new(PipelineStats::default())),
            peer_id,
            info_dict,
        }
    }

    /// Connect to a peer and perform the handshake
    pub async fn connect_to_peer(
        &self,
        peer: &Peer,
    ) -> Result<TcpStream, Box<dyn std::error::Error>> {
        let addr = SocketAddr::from((peer.ip_addr, peer.port));
        let stream = timeout(CONNECTION_TIMEOUT, TcpStream::connect(addr)).await??;

        let handshake = {
            let download_state = self.download_state.lock().await;
            Handshake {
                infohash: download_state.info_hash,
                peer_id: self.peer_id,
            }
        };

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
        stream
            .write_all(&metadata::create_extension_handshake().serialize())
            .await?;
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
                            if let Err(e) = self.process_message(msg, &addr).await {
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

            if let Err(e) = self.flush_upload_queue(&addr, &mut write_half).await {
                debug!("Error flushing upload queue for peer {addr}: {e}");
                break;
            }

            if let Err(e) = self.flush_metadata_queue(&addr, &mut write_half).await {
                debug!("Error flushing metadata queue for peer {addr}: {e}");
                break;
            }
            // Print diagnostics every 60 seconds
            let should_diagnose = {
                let connections = self.connections.lock().await;
                connections
                    .get(&addr)
                    .map(|conn| {
                        conn.last_request_time
                            .map(|t| t.elapsed().as_secs() > 60)
                            .unwrap_or(true)
                    })
                    .unwrap_or(false)
            };
            if should_diagnose {
                self.print_peer_diagnostics().await;
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
                self.handle_piece_request(message, addr).await?;
            }
            MessageId::Cancel => {
                if let Ok(cancel) = PieceRequest::try_from(&message) {
                    let mut connections = self.connections.lock().await;
                    if let Some(conn) = connections.get_mut(addr) {
                        conn.pending_uploads.retain(|p| {
                            p.piece_index != cancel.piece_index
                                || p.begin != cancel.begin
                                || p.length != cancel.length
                        });
                    }
                }
            }
            MessageId::Extended => {
                self.handle_extended_message(message, addr).await?;
            }
        }
        Ok(())
    }

    async fn handle_piece_request(
        &self,
        message: Message,
        addr: &SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Ok(piece_request) = PieceRequest::try_from(&message) {
            if piece_request.length > 16 * 1024 {
                return Ok(());
            }
            let mut connections = self.connections.lock().await;
            if let Some(conn) = connections.get_mut(addr)
                && !conn.am_choking
                && conn.can_queue_upload()
            {
                conn.pending_uploads.push_back(piece_request);
            }
        }
        Ok(())
    }

    /// Dispatch an incoming `Extended` message (BEP-10).
    ///
    /// Two sub-cases, distinguished by `message.payload[0]`:
    ///
    /// **Handshake (`0`)** — the peer's extension capability announcement.
    /// Parse the bencode dict (`payload[1..]`) to find `"m" → "ut_metadata"`.
    /// Store that value in `conn.peer_ut_metadata_id` so you can address
    /// data/reject responses correctly.
    ///
    /// **Metadata request (`OUR_UT_METADATA_ID`)** — the peer is asking for a
    /// piece of our info dict (BEP-9 `msg_type=0`).
    /// Call `parse_metadata_request` to get the piece index, then either:
    ///   - slice the info dict and queue `create_metadata_data_response`, or
    ///   - queue `create_metadata_reject` if the dict is unavailable.
    ///
    /// Hint: `BitTorrentClient` will need an `info_dict: Option<Arc<Vec<u8>>>`
    /// field, and `PeerConnection` a `pending_metadata_responses` queue mirroring
    /// `pending_uploads`, drained by a new `flush_metadata_queue` step in the
    /// main loop.
    async fn handle_extended_message(
        &self,
        message: Message,
        addr: &SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match ExtendedMessageId::try_from(message.payload.first().copied().unwrap_or(255)) {
            Ok(ExtendedMessageId::Handshake) => {
                if let Some(ut_id) = metadata::parse_handshake_ut_id(&message.payload) {
                    let mut connections = self.connections.lock().await;
                    if let Some(conn) = connections.get_mut(addr) {
                        conn.peer_ut_metadata_id = Some(ut_id);
                        Ok(())
                    } else {
                        Err("No connection found for handshake".into())
                    }
                } else {
                    Err("ut_metadata not found in handshake".into())
                }
            }
            Ok(ExtendedMessageId::Metadata) => {
                let Some(piece_index) = metadata::parse_metadata_request(
                    &message.payload,
                    metadata::OUR_UT_METADATA_ID,
                ) else {
                    return Ok(());
                };
                let (_peer_ut_id, response) = {
                    let connections = self.connections.lock().await;
                    let conn = connections.get(addr).ok_or("Unknown peer")?;
                    let peer_ut_id = conn.peer_ut_metadata_id.unwrap_or(1);
                    let response = if let Some(ref info_dict) = self.info_dict {
                        let start = piece_index as usize * 16384;
                        if start >= info_dict.len() {
                            metadata::create_metadata_reject(peer_ut_id, piece_index)
                        } else {
                            let end = (start + metadata::METADATA_PIECE_SIZE).min(info_dict.len());
                            metadata::create_metadata_data_response(
                                peer_ut_id,
                                piece_index,
                                &info_dict[start..end],
                                info_dict.len(),
                            )
                        }
                    } else {
                        metadata::create_metadata_reject(peer_ut_id, piece_index)
                    };
                    (peer_ut_id, response)
                };
                let mut connections = self.connections.lock().await;
                if let Some(conn) = connections.get_mut(addr) {
                    conn.pending_metadata_responses.push_back(response);
                    Ok(())
                } else {
                    Err("Unknown peer".into())
                }
            }
            _ => {
                Err("Unknown extended message".into())
            }
        }
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
        let (can_download, peer_state, should_log) = {
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
                let should_log = conn
                    .last_request_time
                    .map(|last| last.elapsed().as_secs() > 10)
                    .unwrap_or(true);
                (can_download, state, should_log)
            } else {
                (false, "no connection".to_string(), true)
            }
        };

        if !can_download {
            if should_log {
                debug!("🚫 Peer {addr} not ready: {peer_state}");
            }
            return Ok(());
        }

        self.try_download_blocks_batched(stream, addr, perf_cache)
            .await?;

        Ok(())
    }

    /// Request a block from a peer
    async fn request_block(
        &self,
        stream: &mut OwnedWriteHalf,
        block_info: BlockInfo,
        pipeline_depth: usize,
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
        let (blocks_to_request, pipeline_depth) = {
            let mut download_state = self.download_state.lock().await;
            let connections = self.connections.lock().await;

            perf_cache.clear();

            let mut current_pending = 0;
            if let Some(conn) = connections.get(addr)
                && let Some(ref bitfield) = conn.bitfield
            {
                perf_cache
                    .bitfield_buffer
                    .extend((0..download_state.total_pieces).map(|i| bitfield.has_piece(i)));

                current_pending = conn.pending_requests.len();
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

            let depth = current_pending + perf_cache.block_batch.len();
            (perf_cache.block_batch.clone(), depth)
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
            if self
                .request_block(stream, block_info.clone(), pipeline_depth)
                .await
                .is_ok()
            {
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
            let mut stats = self.pipeline_stats.lock().await;
            stats.total_requests_sent += requests_sent as u64;
            if pipeline_depth > stats.max_pipeline_depth_seen {
                stats.max_pipeline_depth_seen = pipeline_depth;
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

    /// Flushes the upload queue for the given peer, sending any pending piece requests.
    async fn flush_upload_queue(
        &self,
        addr: &SocketAddr,
        write_half: &mut OwnedWriteHalf,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut to_serve = Vec::new();
        {
            let mut connections = self.connections.lock().await;
            if let Some(conn) = connections.get_mut(addr) {
                for _ in 0..4 {
                    match conn.pending_uploads.pop_front() {
                        Some(req) => to_serve.push(req),
                        None => break,
                    }
                }
            }
        }

        if to_serve.is_empty() {
            return Ok(());
        }

        let mut download_state = self.download_state.lock().await;
        for req in to_serve {
            if let Some(data) = download_state.get_piece_block(
                req.piece_index as usize,
                req.begin as usize,
                req.length as usize,
            ) {
                let msg = build_piece_response(req.piece_index, req.begin, &data);
                write_half.write_all(&msg.serialize()).await?;
                download_state.record_upload(req.length as u64);
            }
        }

        Ok(())
    }

    async fn flush_metadata_queue(
        &self,
        addr: &SocketAddr,
        write_half: &mut OwnedWriteHalf,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut to_serve = Vec::new();
        {
            let mut connections = self.connections.lock().await;
            if let Some(conn) = connections.get_mut(addr) {
                while let Some(req) = conn.pending_metadata_responses.pop_front() {
                    to_serve.push(req);
                }
            }
        }
        if to_serve.is_empty() {
            return Ok(());
        }

        for req in to_serve {
            write_half.write_all(&req.serialize()).await?;
        }

        Ok(())
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
            info_dict: self.info_dict.clone(),
        }
    }
}
