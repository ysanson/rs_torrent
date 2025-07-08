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
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Represents the state of a connection to a peer
#[derive(Debug, Clone)]
pub struct PeerConnection {
    pub peer: Peer,
    pub am_choking: bool,
    pub am_interested: bool,
    pub peer_choking: bool,
    pub peer_interested: bool,
    pub bitfield: Option<Bitfield>,
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
}

/// Main BitTorrent client for downloading from peers
pub struct BitTorrentClient {
    pub download_state: Arc<Mutex<DownloadState>>,
    pub connections: Arc<Mutex<HashMap<SocketAddr, PeerConnection>>>,
    pub peer_id: [u8; 20],
}

impl BitTorrentClient {
    pub fn new(download_state: DownloadState, peer_id: [u8; 20]) -> Self {
        Self {
            download_state: Arc::new(Mutex::new(download_state)),
            connections: Arc::new(Mutex::new(HashMap::new())),
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

        Ok(stream)
    }

    /// Main peer worker that handles communication with a single peer
    pub async fn peer_worker(&self, peer: Peer) -> Result<(), Box<dyn std::error::Error>> {
        let mut stream = self.connect_to_peer(&peer).await?;
        let addr = SocketAddr::from((peer.ip_addr, peer.port));

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

        // Update our state
        {
            let mut connections = self.connections.lock().await;
            if let Some(conn) = connections.get_mut(&addr) {
                conn.am_interested = true;
            }
        }

        // Message handling loop
        loop {
            match self.handle_peer_messages(&mut stream, &addr).await {
                Ok(should_continue) => {
                    if !should_continue {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Error handling peer {}: {}", addr, e);
                    break;
                }
            }

            // Try to download blocks if we can
            if let Err(e) = self.try_download_blocks(&mut stream, &addr).await {
                eprintln!("Error downloading from peer {}: {}", addr, e);
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

        Ok(())
    }

    /// Handle incoming messages from a peer
    async fn handle_peer_messages(
        &self,
        stream: &mut TcpStream,
        addr: &SocketAddr,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        // Read message length
        let mut len_buf = [0u8; 4];
        match timeout(READ_TIMEOUT, stream.read_exact(&mut len_buf)).await {
            Ok(Ok(_)) => {}
            Ok(Err(_)) => return Ok(false), // Connection closed
            Err(_) => return Ok(true),      // Timeout, continue
        }

        let msg_len = u32::from_be_bytes(len_buf) as usize;
        if msg_len == 0 {
            // Keep-alive message
            return Ok(true);
        }

        // Read the full message
        let mut msg_buf = vec![0u8; msg_len];
        timeout(READ_TIMEOUT, stream.read_exact(&mut msg_buf)).await??;

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
                println!("Peer {} unchoked us", addr);
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
                let mut connections = self.connections.lock().await;
                if let Some(conn) = connections.get_mut(addr) {
                    conn.bitfield = Some(bitfield);
                }
                println!("Received bitfield from peer {}", addr);
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

        // Store the block and check if piece is complete
        let piece_complete = {
            let mut download_state = self.download_state.lock().await;
            download_state.complete_block(block_info, block_data.to_vec())
        };

        if piece_complete {
            let download_state = self.download_state.lock().await;
            println!(
                "Completed piece {} ({}/{})",
                piece_index,
                download_state.progress(),
                download_state.total_pieces
            );
        } else {
            let download_state = self.download_state.lock().await;
            let piece_progress = download_state.piece_progress(piece_index);
            println!(
                "Piece {} progress: {:.1}% ({} blocks completed)",
                piece_index,
                piece_progress * 100.0,
                download_state.completed_blocks()
            );
        }

        Ok(())
    }

    /// Try to download blocks from a peer
    async fn try_download_blocks(
        &self,
        stream: &mut TcpStream,
        addr: &SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let can_download = {
            let connections = self.connections.lock().await;
            connections
                .get(addr)
                .map(|conn| conn.can_download())
                .unwrap_or(false)
        };

        if !can_download {
            return Ok(());
        }

        // Find a block to download
        let block_to_download = {
            let download_state = self.download_state.lock().await;
            let connections = self.connections.lock().await;

            if let Some(conn) = connections.get(addr) {
                if let Some(ref bitfield) = conn.bitfield {
                    // Convert bitfield to bool vector for compatibility
                    let peer_bitfield: Vec<bool> = (0..download_state.total_pieces)
                        .map(|i| bitfield.has_piece(i))
                        .collect();

                    download_state.pick_block(&peer_bitfield)
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(block_info) = block_to_download {
            // Mark as requested
            {
                let mut download_state = self.download_state.lock().await;
                download_state.mark_block_requested(block_info.clone());
            }

            // Request the block
            self.request_block(stream, block_info).await?;
        }

        Ok(())
    }

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
        println!(
            "Requested block: piece {}, offset {}, length {} from peer",
            block_info.piece_index, block_info.offset, block_info.length
        );
        Ok(())
    }

    /// Request a piece from a peer (legacy method - now requests all blocks)
    async fn request_piece(
        &self,
        stream: &mut TcpStream,
        piece_index: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let piece_length = {
            let download_state = self.download_state.lock().await;
            download_state.piece_length as usize
        };

        // Request all blocks in the piece
        let mut offset = 0;
        while offset < piece_length {
            let block_length = (piece_length - offset).min(PIECE_BLOCK_SIZE);
            let block_info = BlockInfo {
                piece_index,
                offset,
                length: block_length,
            };

            // Mark as requested
            {
                let mut download_state = self.download_state.lock().await;
                download_state.mark_block_requested(block_info.clone());
            }

            // Request the block
            self.request_block(stream, block_info).await?;

            offset += block_length;
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
}

impl Clone for BitTorrentClient {
    fn clone(&self) -> Self {
        Self {
            download_state: Arc::clone(&self.download_state),
            connections: Arc::clone(&self.connections),
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
    }
}
