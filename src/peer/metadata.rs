use crate::bencode_parser::parser::{parse_owned, ValueOwned};
use crate::peer::handshake::Handshake;
use crate::peer::message::{ExtendedMessageId, Message, MessageId};
use crate::peer::Peer;
use log::debug;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const METADATA_PIECE_SIZE: usize = 16384; // 16 KiB per piece
const MAX_RETRIES: usize = 3;

/// Represents the metadata extension handshake information
#[derive(Debug, Clone)]
pub struct MetadataExtension {
    pub ut_metadata: u8,
    pub metadata_size: usize,
}

/// Fetch metadata from a list of peers
pub async fn fetch_metadata_from_peers(
    peers: Vec<Peer>,
    infohash: &[u8; 20],
    peer_id: [u8; 20],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    for peer in peers {
        println!("🔗 Attempting to fetch metadata from {}:{}", peer.ip_addr, peer.port);
        
        match fetch_metadata_from_peer(&peer, infohash, peer_id).await {
            Ok(metadata) => {
                println!("✅ Successfully fetched metadata from {}:{}", peer.ip_addr, peer.port);
                return Ok(metadata);
            }
            Err(e) => {
                debug!("Failed to fetch metadata from {}:{}: {}", peer.ip_addr, peer.port, e);
                continue;
            }
        }
    }
    
    Err("Failed to fetch metadata from any peer".into())
}

/// Fetch metadata from a single peer
async fn fetch_metadata_from_peer(
    peer: &Peer,
    infohash: &[u8; 20],
    peer_id: [u8; 20],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let addr = SocketAddr::from((peer.ip_addr, peer.port));
    let mut stream = timeout(CONNECTION_TIMEOUT, TcpStream::connect(addr)).await??;
    
    // Perform handshake with extension support
    let handshake = Handshake {
        infohash: *infohash,
        peer_id,
    };
    
    // Send handshake with extension bit set (bit 20 in reserved bytes)
    let mut handshake_bytes = handshake.serialize();
    handshake_bytes[25] |= 0x10; // Set extension protocol bit
    stream.write_all(&handshake_bytes).await?;
    
    // Receive peer's handshake
    let mut response = [0u8; 68];
    timeout(READ_TIMEOUT, stream.read_exact(&mut response)).await??;
    
    let peer_handshake = Handshake::deserialize(&response)
        .ok_or("Invalid handshake response")?;
    
    if peer_handshake.infohash != *infohash {
        return Err("Infohash mismatch".into());
    }
    
    // Check if peer supports extensions
    if response[25] & 0x10 == 0 {
        return Err("Peer does not support extension protocol".into());
    }
    
    debug!("✅ Handshake successful, peer supports extensions");
    
    // Handle any initial messages (bitfield, etc.) before sending extension handshake
    // Give peer a moment to send initial messages
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Try to read and discard any pending messages (non-blocking)
    let _ = drain_pending_messages(&mut stream).await;
    
    // Send extended handshake
    let ext_handshake = create_extension_handshake();
    stream.write_all(&ext_handshake.serialize()).await?;
    debug!("📤 Sent extension handshake");
    
    // Wait for peer's extended handshake
    let metadata_ext = receive_extension_handshake(&mut stream).await?;
    debug!("📥 Received extension handshake: ut_metadata={}, size={}", 
           metadata_ext.ut_metadata, metadata_ext.metadata_size);
    
    // Calculate number of pieces needed
    let num_pieces = (metadata_ext.metadata_size + METADATA_PIECE_SIZE - 1) / METADATA_PIECE_SIZE;
    let mut metadata_pieces: Vec<Option<Vec<u8>>> = vec![None; num_pieces];
    let mut requested_pieces: Vec<bool> = vec![false; num_pieces];
    
    println!("📦 Metadata size: {} bytes, {} pieces", metadata_ext.metadata_size, num_pieces);
    
    // Request and receive metadata pieces one at a time
    let mut received_pieces = 0;
    let mut retry_count = 0;
    
    while received_pieces < num_pieces && retry_count < MAX_RETRIES {
        // Request next unreceived piece
        for piece_index in 0..num_pieces {
            if metadata_pieces[piece_index].is_none() && !requested_pieces[piece_index] {
                let request = create_metadata_request(metadata_ext.ut_metadata, piece_index as u32);
                stream.write_all(&request.serialize()).await?;
                requested_pieces[piece_index] = true;
                debug!("📤 Requested metadata piece {}/{}", piece_index + 1, num_pieces);
                
                // Only request one piece at a time, wait for response
                break;
            }
        }
        
        // Wait for response
        match receive_message_with_retry(&mut stream).await {
            Ok(message) => {
                match message.kind {
                    MessageId::Extended => {
                        if let Some((piece_index, piece_data)) = 
                            parse_metadata_piece(&message.payload, metadata_ext.ut_metadata)? {
                            
                            if piece_index < num_pieces && metadata_pieces[piece_index].is_none() {
                                metadata_pieces[piece_index] = Some(piece_data);
                                received_pieces += 1;
                                retry_count = 0; // Reset retry count on success
                                println!("📥 Received metadata piece {}/{}", received_pieces, num_pieces);
                            }
                        }
                    }
                    MessageId::Bitfield | MessageId::Have | MessageId::Unchoke | 
                    MessageId::Choke | MessageId::Interested | MessageId::NotInterested => {
                        // Ignore these messages, continue waiting for metadata
                        debug!("Received {:?} message, continuing to wait for metadata", message.kind);
                    }
                    _ => {
                        debug!("Received unexpected message type: {:?}", message.kind);
                    }
                }
            }
            Err(e) => {
                debug!("Error receiving message: {}", e);
                retry_count += 1;
                
                // Reset requested flags for unreceived pieces so we can retry
                for i in 0..num_pieces {
                    if metadata_pieces[i].is_none() {
                        requested_pieces[i] = false;
                    }
                }
                
                if retry_count >= MAX_RETRIES {
                    break;
                }
                
                // Small delay before retry
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    
    // Assemble complete metadata
    if received_pieces == num_pieces {
        let mut complete_metadata = Vec::new();
        for piece in metadata_pieces {
            if let Some(data) = piece {
                complete_metadata.extend_from_slice(&data);
            } else {
                return Err("Missing metadata piece".into());
            }
        }
        
        // Trim to exact size
        complete_metadata.truncate(metadata_ext.metadata_size);
        
        // Verify the metadata by checking if it's valid bencode
        parse_owned(&complete_metadata)?;
        
        Ok(complete_metadata)
    } else {
        Err(format!("Only received {}/{} metadata pieces after {} retries", 
                    received_pieces, num_pieces, retry_count).into())
    }
}

/// Drain any pending messages from the stream (non-blocking)
async fn drain_pending_messages(stream: &mut TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    // Try to read with a very short timeout
    loop {
        match timeout(Duration::from_millis(50), receive_message(stream)).await {
            Ok(Ok(msg)) => {
                debug!("Drained message: {:?}", msg.kind);
            }
            _ => break, // Timeout or error, no more messages
        }
    }
    Ok(())
}

/// Create an extension handshake message
fn create_extension_handshake() -> Message {
    // Create bencode dictionary: d1:md11:ut_metadatai1eee
    // This means: {"m": {"ut_metadata": 1}}
    let payload = b"d1:md11:ut_metadatai1eee";
    
    let mut full_payload = Vec::new();
    full_payload.push(ExtendedMessageId::Handshake as u8);
    full_payload.extend_from_slice(payload);
    
    Message {
        kind: MessageId::Extended,
        payload: full_payload,
    }
}

/// Receive and parse extension handshake from peer
async fn receive_extension_handshake(
    stream: &mut TcpStream,
) -> Result<MetadataExtension, Box<dyn std::error::Error>> {
    // Keep trying to receive until we get an extension handshake
    loop {
        let message = receive_message(stream).await?;
        
        if message.kind == MessageId::Extended {
            if !message.payload.is_empty() && message.payload[0] == ExtendedMessageId::Handshake as u8 {
                // Parse bencode dictionary
                let bencode_data = &message.payload[1..];
                let parsed = parse_owned(bencode_data)?;
                
                if let Some(ValueOwned::Dictionary { entries, .. }) = parsed.first() {
                    // Get the "m" dictionary
                    if let Some(ValueOwned::Dictionary { entries: m_dict, .. }) = entries.get(b"m" as &[u8]) {
                        // Get ut_metadata value
                        if let Some(ValueOwned::Integer(ut_metadata)) = m_dict.get(b"ut_metadata" as &[u8]) {
                            // Get metadata_size
                            if let Some(ValueOwned::Integer(metadata_size)) = entries.get(b"metadata_size" as &[u8]) {
                                return Ok(MetadataExtension {
                                    ut_metadata: *ut_metadata as u8,
                                    metadata_size: *metadata_size as usize,
                                });
                            }
                        }
                    }
                }
                
                return Err("Invalid extension handshake format".into());
            }
        } else {
            // Not an extension handshake, might be bitfield or other message
            debug!("Received {:?} while waiting for extension handshake, continuing...", message.kind);
        }
    }
}

/// Create a metadata request message
fn create_metadata_request(ut_metadata: u8, piece_index: u32) -> Message {
    // Create bencode dictionary: d8:msg_typei0e5:piecei<piece>ee
    // msg_type 0 = request
    let bencode = format!("d8:msg_typei0e5:piecei{}ee", piece_index);
    
    let mut payload = Vec::new();
    payload.push(ut_metadata);
    payload.extend_from_slice(bencode.as_bytes());
    
    Message {
        kind: MessageId::Extended,
        payload,
    }
}

/// Parse a metadata piece from an extended message
fn parse_metadata_piece(
    payload: &[u8],
    expected_ut_metadata: u8,
) -> Result<Option<(usize, Vec<u8>)>, Box<dyn std::error::Error>> {
    if payload.is_empty() {
        return Ok(None);
    }
    
    let ut_metadata = payload[0];
    if ut_metadata != expected_ut_metadata {
        return Ok(None);
    }
    
    // Find the end of the bencode dictionary
    let bencode_data = &payload[1..];
    let mut bencode_end = 0;
    let mut depth = 0;
    
    for (i, &byte) in bencode_data.iter().enumerate() {
        match byte {
            b'd' | b'l' => depth += 1,
            b'e' => {
                depth -= 1;
                if depth == 0 {
                    bencode_end = i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    
    if bencode_end == 0 {
        return Err("Invalid metadata piece format".into());
    }
    
    // Parse the bencode header
    let header = &bencode_data[..bencode_end];
    let parsed = parse_owned(header)?;
    
    if let Some(ValueOwned::Dictionary { entries, .. }) = parsed.first() {
        // Check msg_type (should be 1 for data)
        if let Some(ValueOwned::Integer(msg_type)) = entries.get(b"msg_type" as &[u8]) {
            if *msg_type == 2 {
                // msg_type 2 = reject
                debug!("Peer rejected metadata request");
                return Err("Metadata request rejected by peer".into());
            }
            if *msg_type != 1 {
                // Not a data response
                return Ok(None);
            }
        }
        
        // Get piece index
        if let Some(ValueOwned::Integer(piece)) = entries.get(b"piece" as &[u8]) {
            let piece_index = *piece as usize;
            
            // The actual data follows the bencode dictionary
            let data = bencode_data[bencode_end..].to_vec();
            
            debug!("Parsed metadata piece {}, data length: {}", piece_index, data.len());
            
            return Ok(Some((piece_index, data)));
        }
    }
    
    Ok(None)
}

/// Receive a complete message from the stream with retry on keep-alive
async fn receive_message_with_retry(stream: &mut TcpStream) -> Result<Message, Box<dyn std::error::Error>> {
    loop {
        match receive_message(stream).await {
            Ok(msg) => return Ok(msg),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("keep-alive") {
                    // Keep-alive received, continue waiting
                    debug!("Received keep-alive, continuing...");
                    continue;
                } else {
                    return Err(e);
                }
            }
        }
    }
}

/// Receive a complete message from the stream
async fn receive_message(stream: &mut TcpStream) -> Result<Message, Box<dyn std::error::Error>> {
    // Read message length
    let mut len_buf = [0u8; 4];
    timeout(READ_TIMEOUT, stream.read_exact(&mut len_buf)).await??;
    
    let msg_len = u32::from_be_bytes(len_buf) as usize;
    if msg_len == 0 {
        // Keep-alive message
        return Err("Received keep-alive".into());
    }
    
    if msg_len > 1024 * 1024 {
        // Sanity check: reject messages larger than 1MB
        return Err(format!("Message too large: {} bytes", msg_len).into());
    }
    
    // Read message
    let mut msg_buf = vec![0u8; msg_len];
    timeout(READ_TIMEOUT, stream.read_exact(&mut msg_buf)).await??;
    
    // Reconstruct full message
    let mut full_msg = Vec::with_capacity(4 + msg_len);
    full_msg.extend_from_slice(&len_buf);
    full_msg.extend_from_slice(&msg_buf);
    
    Message::deserialize(&full_msg).ok_or("Failed to deserialize message".into())
}
