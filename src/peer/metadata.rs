use crate::bencode_parser::parser::{ValueOwned, parse_owned};
use crate::peer::Peer;
use crate::peer::handshake::Handshake;
use crate::peer::message::{ExtendedMessage, ExtendedMessageId, Message, MessageId};
use crate::torrent::Infohash;
use log::debug;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
pub const METADATA_PIECE_SIZE: usize = 16384; // 16 KiB per piece
const MAX_RETRIES: usize = 3;
// The local ID we advertise for ut_metadata in our extension handshake.
// Peers use this ID as payload[0] when sending metadata pieces back to us.
pub const OUR_UT_METADATA_ID: u8 = 1;

/// Represents the metadata extension handshake information
#[derive(Debug, Clone)]
pub struct MetadataExtension {
    pub ut_metadata: u8,
    pub metadata_size: usize,
}

pub type MetadataPiece = (usize, Vec<u8>);

/// Fetch metadata from a list of peers, trying up to N peers concurrently.
pub async fn fetch_metadata_from_peers(
    peers: &[Peer],
    infohash: &Infohash,
    peer_id: [u8; 20],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    const CONCURRENT: usize = 5;

    let mut tasks: tokio::task::JoinSet<Result<Vec<u8>, String>> = tokio::task::JoinSet::new();
    let mut peers_iter = peers.iter();

    for _ in 0..CONCURRENT {
        spawn_next_peer(&mut tasks, &mut peers_iter, infohash, peer_id);
    }

    loop {
        match tasks.join_next().await {
            Some(Ok(Ok(metadata))) => {
                tasks.abort_all();
                return Ok(metadata);
            }
            Some(Ok(Err(e))) => {
                debug!("Peer failed: {e}");
                spawn_next_peer(&mut tasks, &mut peers_iter, infohash, peer_id);
            }
            Some(Err(_)) => {
                spawn_next_peer(&mut tasks, &mut peers_iter, infohash, peer_id);
            }
            None => return Err("Failed to fetch metadata from any peer".into()),
        }
    }
}

fn spawn_next_peer(
    tasks: &mut tokio::task::JoinSet<Result<Vec<u8>, String>>,
    peers_iter: &mut std::slice::Iter<'_, Peer>,
    infohash: &Infohash,
    peer_id: [u8; 20],
) {
    if let Some(peer) = peers_iter.next() {
        let peer = peer.clone();
        let infohash = *infohash;
        println!(
            "🔗 Attempting to fetch metadata from {}:{}",
            peer.ip_addr, peer.port
        );
        tasks.spawn(async move {
            fetch_metadata_from_peer(&peer, &infohash, peer_id)
                .await
                .map_err(|e| e.to_string())
        });
    }
}

/// Fetch metadata from a single peer
async fn fetch_metadata_from_peer(
    peer: &Peer,
    infohash: &Infohash,
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

    let peer_handshake = Handshake::deserialize(&response).ok_or("Invalid handshake response")?;

    if peer_handshake.infohash != *infohash {
        return Err("Infohash mismatch".into());
    }

    // Check if peer supports extensions
    if response[25] & 0x10 == 0 {
        return Err("Peer does not support extension protocol".into());
    }

    debug!("✅ Handshake successful, peer supports extensions");

    // Send extended handshake immediately after initial handshake
    let ext_handshake = create_extension_handshake();
    stream.write_all(&ext_handshake.serialize()).await?;
    debug!("📤 Sent extension handshake");

    // Wait for peer's extended handshake
    let metadata_ext = receive_extension_handshake(&mut stream).await?;
    debug!(
        "📥 Received extension handshake: ut_metadata={}, size={}",
        metadata_ext.ut_metadata, metadata_ext.metadata_size
    );

    // Calculate number of pieces needed
    let num_pieces = metadata_ext.metadata_size.div_ceil(METADATA_PIECE_SIZE);
    let mut metadata_pieces: Vec<Option<Vec<u8>>> = vec![None; num_pieces];
    let mut requested_pieces: Vec<bool> = vec![false; num_pieces];

    println!(
        "📦 Metadata size: {} bytes, {} pieces",
        metadata_ext.metadata_size, num_pieces
    );

    // Request and receive metadata pieces one at a time
    let mut received_pieces = 0;
    let mut retry_count = 0;
    let mut waiting_for_response = false;

    while received_pieces < num_pieces && retry_count < MAX_RETRIES {
        // Request next unreceived piece only if not already waiting for a response
        if !waiting_for_response {
            for piece_index in 0..num_pieces {
                if metadata_pieces[piece_index].is_none() && !requested_pieces[piece_index] {
                    let request =
                        create_metadata_request(metadata_ext.ut_metadata, piece_index as u32);
                    stream.write_all(&request.serialize()).await?;
                    requested_pieces[piece_index] = true;
                    waiting_for_response = true;
                    debug!(
                        "📤 Requested metadata piece {}/{}",
                        piece_index + 1,
                        num_pieces
                    );

                    // Only request one piece at a time, wait for response
                    break;
                }
            }
        }

        // Wait for response; track whether we should sleep before the next attempt.
        let mut sleep_before_retry = false;
        match receive_message_with_retry(&mut stream).await {
            Ok(message) => match message.kind {
                MessageId::Extended => {
                    if let Some((piece_index, piece_data)) =
                        parse_metadata_piece(&message.payload, OUR_UT_METADATA_ID)?
                        && piece_index < num_pieces
                        && metadata_pieces[piece_index].is_none()
                    {
                        metadata_pieces[piece_index] = Some(piece_data);
                        received_pieces += 1;
                        waiting_for_response = false;
                        retry_count = 0;
                        println!(
                            "📥 Received metadata piece {}/{}",
                            received_pieces, num_pieces
                        );
                    }
                }
                MessageId::Bitfield
                | MessageId::Have
                | MessageId::Unchoke
                | MessageId::Choke
                | MessageId::Interested
                | MessageId::NotInterested => {
                    debug!(
                        "Received {:?} message, continuing to wait for metadata",
                        message.kind
                    );
                }
                _ => {
                    debug!("Received unexpected message type: {:?}", message.kind);
                }
            },
            Err(e) => {
                // e must be fully consumed before any .await; convert to string immediately.
                debug!("Error receiving message: {}", e);
                retry_count += 1;
                waiting_for_response = false;
                for i in 0..num_pieces {
                    if metadata_pieces[i].is_none() {
                        requested_pieces[i] = false;
                    }
                }
                if retry_count >= MAX_RETRIES {
                    break;
                }
                sleep_before_retry = true;
            } // e is dropped here, before any .await
        }
        if sleep_before_retry {
            tokio::time::sleep(Duration::from_millis(500)).await;
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
        Err(format!(
            "Only received {}/{} metadata pieces after {} retries",
            received_pieces, num_pieces, retry_count
        )
        .into())
    }
}

/// Create an extension handshake message
pub fn create_extension_handshake() -> Message {
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
        let message = loop {
            match receive_message(stream).await {
                Ok(msg) => break msg,
                Err(e) if e.to_string().contains("keep-alive") => continue,
                Err(e) => return Err(e),
            }
        };

        if message.kind == MessageId::Extended {
            if !message.payload.is_empty()
                && message.payload[0] == ExtendedMessageId::Handshake as u8
            {
                // Parse bencode dictionary
                let bencode_data = &message.payload[1..];
                let parsed = parse_owned(bencode_data)?;

                if let Some(ValueOwned::Dictionary { entries, .. }) = parsed.first() {
                    // Get the "m" dictionary
                    if let Some(ValueOwned::Dictionary {
                        entries: m_dict, ..
                    }) = entries.get(b"m" as &[u8])
                    {
                        // Get ut_metadata value
                        if let Some(ValueOwned::Integer(ut_metadata)) =
                            m_dict.get(b"ut_metadata" as &[u8])
                        {
                            // Get metadata_size
                            if let Some(ValueOwned::Integer(metadata_size)) =
                                entries.get(b"metadata_size" as &[u8])
                            {
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
            debug!(
                "Received {:?} while waiting for extension handshake, continuing...",
                message.kind
            );
        }
    }
}

/// Create a metadata request message
pub fn create_metadata_request(ut_metadata: u8, piece_index: u32) -> Message {
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

/// Parse a raw extended message payload and return the inner data if it is a metadata message.
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

/// Find the byte length of the first complete bencode value in `data`.
/// Unlike a naive depth counter, this correctly skips string payloads so
/// that bytes like 'e' inside a string (e.g. "msg_type") are never mistaken
/// for end-of-container markers.
fn find_bencode_end(data: &[u8]) -> Option<usize> {
    let mut pos = 0;
    let mut depth: i32 = 0;

    loop {
        if pos >= data.len() {
            return None;
        }
        match data[pos] {
            b'd' | b'l' => {
                depth += 1;
                pos += 1;
            }
            b'i' => {
                pos += 1;
                while pos < data.len() && data[pos] != b'e' {
                    pos += 1;
                }
                if pos >= data.len() {
                    return None;
                }
                pos += 1; // consume 'e'
                if depth == 0 {
                    return Some(pos);
                }
            }
            b'e' => {
                depth -= 1;
                pos += 1;
                if depth == 0 {
                    return Some(pos);
                }
            }
            b'0'..=b'9' => {
                let start = pos;
                while pos < data.len() && data[pos] != b':' {
                    pos += 1;
                }
                if pos >= data.len() {
                    return None;
                }
                let len: usize = std::str::from_utf8(&data[start..pos]).ok()?.parse().ok()?;
                pos += 1 + len; // consume ':' and the string bytes
                if depth == 0 {
                    return Some(pos);
                }
            }
            _ => return None,
        }
    }
}

/// Parse a metadata piece from an extended message
fn parse_metadata_piece(
    payload: &[u8],
    expected_ut_metadata: u8,
) -> Result<Option<MetadataPiece>, Box<dyn std::error::Error>> {
    if payload.is_empty() {
        return Ok(None);
    }

    let ut_metadata = payload[0];
    if ut_metadata != expected_ut_metadata {
        return Ok(None);
    }

    let bencode_data = &payload[1..];
    let bencode_end = find_bencode_end(bencode_data).ok_or("Invalid metadata piece format")?;

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

            debug!(
                "Parsed metadata piece {}, data length: {}",
                piece_index,
                data.len()
            );

            return Ok(Some((piece_index, data)));
        }
    }

    Ok(None)
}

/// Receive a complete message from the stream with retry on keep-alive
async fn receive_message_with_retry(
    stream: &mut TcpStream,
) -> Result<Message, Box<dyn std::error::Error>> {
    loop {
        match receive_message(stream).await {
            Ok(msg) => return Ok(msg),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("keep-alive") {
                    // Keep-alive received, continuing...
                    debug!("Received keep-alive, continuing...");
                    continue;
                } else {
                    return Err(e);
                }
            }
        }
    }
}

// ── Responding to incoming metadata requests (BEP-9 server side) ─────────────

/// Parse an incoming metadata request from a peer's extended message payload.
///
/// Payload layout (as received in an `Extended` message):
/// ```text
/// byte  0:    ut_metadata ID we advertised — must equal `expected_ut_metadata`
/// bytes 1…:  bencode dict  {"msg_type": 0, "piece": N}
/// ```
///
/// Returns the requested piece index `N` if the message is a valid `msg_type=0`
/// request and the leading ID byte matches.  Returns `None` for anything else
/// (wrong ID, wrong msg_type, malformed bencode).
pub fn parse_metadata_request(payload: &[u8], expected_ut_metadata: u8) -> Option<u32> {
    if payload.is_empty() {
        return None;
    }
    let ut_metadata_id = payload[0];
    if ut_metadata_id != expected_ut_metadata {
        return None;
    }
    let bencode_data = &payload[1..];
    let bencode_end = find_bencode_end(bencode_data)?;
    let parsed = parse_owned(&bencode_data[..bencode_end]).ok()?;
    let ValueOwned::Dictionary { entries, .. } = parsed.first()? else {
        return None;
    };

    match entries.get(b"msg_type" as &[u8]) {
        Some(ValueOwned::Integer(0)) => {}
        _ => return None,
    }
    match entries.get(b"piece" as &[u8]) {
        Some(ValueOwned::Integer(n)) => Some(*n as u32),
        _ => None,
    }
}

/// Build a metadata *data* response (BEP-9 `msg_type=1`) for one piece.
///
/// Payload layout (to be wrapped in an `Extended` message):
/// ```text
/// byte  0:    peer_ut_metadata_id  — the ID the peer told us to use in their handshake
/// bytes 1…:  bencode dict  {"msg_type": 1, "piece": N, "total_size": T}
/// trailing:  raw bytes for this piece of the info dict
/// ```
///
/// `piece_data` is a slice of the full info dict covering exactly this piece
/// (at most 16 KiB).  `total_size` is the byte length of the complete info dict.
pub fn create_metadata_data_response(
    peer_ut_metadata_id: u8,
    piece_index: u32,
    piece_data: &[u8],
    total_size: usize,
) -> Message {
    let bencode = format!(
        "d8:msg_typei1e5:piecei{}e10:total_sizei{}ee",
        piece_index, total_size
    );
    let payload = [
        [peer_ut_metadata_id].as_ref(),
        bencode.as_bytes(),
        piece_data,
    ]
    .concat();
    Message {
        kind: MessageId::Extended,
        payload,
    }
}

/// Build a metadata *reject* message (BEP-9 `msg_type=2`).
///
/// Payload layout:
/// ```text
/// byte  0:    peer_ut_metadata_id  — the ID the peer told us to use
/// bytes 1…:  bencode dict  {"msg_type": 2, "piece": N}
/// ```
///
/// Send this when you cannot serve the requested piece (info dict not yet
/// available, or piece index is out of range).
pub fn create_metadata_reject(peer_ut_metadata_id: u8, piece_index: u32) -> Message {
    let bencode = format!("d8:msg_typei2e5:piecei{}ee", piece_index);
    let payload = [[peer_ut_metadata_id].as_ref(), bencode.as_bytes()].concat();
    Message {
        kind: MessageId::Extended,
        payload,
    }
}

pub fn parse_handshake_ut_id(payload: &[u8]) -> Option<u8> {
    let parsed = parse_owned(&payload[1..]).ok()?;
    let ValueOwned::Dictionary { entries, .. } = parsed.into_iter().next()? else {
        return None;
    };
    let ValueOwned::Dictionary {
        entries: m_dict, ..
    } = entries.get(b"m" as &[u8])?
    else {
        return None;
    };
    let ValueOwned::Integer(n) = m_dict.get(b"ut_metadata" as &[u8])? else {
        return None;
    };
    Some(*n as u8)
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode_parser::parser::{ValueOwned, parse_owned};
    use crate::peer::message::MessageId;

    /// Build the raw payload a peer would send when requesting metadata piece `piece`.
    fn make_request_payload(ut_id: u8, piece: u32) -> Vec<u8> {
        let bencode = format!("d8:msg_typei0e5:piecei{}ee", piece);
        let mut buf = vec![ut_id];
        buf.extend_from_slice(bencode.as_bytes());
        buf
    }

    // ── parse_metadata_request ────────────────────────────────────────────────

    #[test]
    fn parse_metadata_request_returns_piece_index() {
        let payload = make_request_payload(1, 0);
        assert_eq!(parse_metadata_request(&payload, 1), Some(0));
    }

    #[test]
    fn parse_metadata_request_non_zero_piece() {
        let payload = make_request_payload(1, 5);
        assert_eq!(parse_metadata_request(&payload, 1), Some(5));
    }

    #[test]
    fn parse_metadata_request_wrong_ut_id_returns_none() {
        let payload = make_request_payload(2, 0);
        assert_eq!(parse_metadata_request(&payload, 1), None);
    }

    #[test]
    fn parse_metadata_request_empty_returns_none() {
        assert_eq!(parse_metadata_request(&[], 1), None);
    }

    #[test]
    fn parse_metadata_request_data_msg_type_returns_none() {
        // msg_type=1 is a data response — must not be parsed as a request
        let bencode = b"d8:msg_typei1e5:piecei0ee";
        let mut payload = vec![1u8];
        payload.extend_from_slice(bencode);
        assert_eq!(parse_metadata_request(&payload, 1), None);
    }

    #[test]
    fn parse_metadata_request_reject_msg_type_returns_none() {
        let bencode = b"d8:msg_typei2e5:piecei0ee";
        let mut payload = vec![1u8];
        payload.extend_from_slice(bencode);
        assert_eq!(parse_metadata_request(&payload, 1), None);
    }

    // ── create_metadata_reject ────────────────────────────────────────────────

    #[test]
    fn create_metadata_reject_is_extended_message() {
        let msg = create_metadata_reject(1, 0);
        assert_eq!(msg.kind, MessageId::Extended);
    }

    #[test]
    fn create_metadata_reject_uses_peer_ut_id() {
        let msg = create_metadata_reject(3, 0);
        assert_eq!(
            msg.payload[0], 3,
            "first byte must be the peer's ut_metadata ID"
        );
    }

    #[test]
    fn create_metadata_reject_encodes_msg_type_and_piece() {
        let msg = create_metadata_reject(1, 7);
        let bencode = &msg.payload[1..];
        let parsed = parse_owned(bencode).expect("reject payload must be valid bencode");
        let dict = match parsed.first() {
            Some(ValueOwned::Dictionary { entries, .. }) => entries,
            _ => panic!("expected a bencode dictionary"),
        };
        match dict.get(b"msg_type" as &[u8]) {
            Some(ValueOwned::Integer(v)) => assert_eq!(*v, 2, "msg_type must be 2 (reject)"),
            _ => panic!("missing or wrong msg_type"),
        }
        match dict.get(b"piece" as &[u8]) {
            Some(ValueOwned::Integer(v)) => assert_eq!(*v, 7),
            _ => panic!("missing or wrong piece"),
        }
    }

    // ── create_metadata_data_response ─────────────────────────────────────────

    #[test]
    fn create_metadata_data_response_is_extended_message() {
        let msg = create_metadata_data_response(1, 0, b"data", 4);
        assert_eq!(msg.kind, MessageId::Extended);
    }

    #[test]
    fn create_metadata_data_response_uses_peer_ut_id() {
        let msg = create_metadata_data_response(4, 0, b"data", 4);
        assert_eq!(
            msg.payload[0], 4,
            "first byte must be the peer's ut_metadata ID"
        );
    }

    #[test]
    fn create_metadata_data_response_appends_piece_bytes_at_end() {
        let piece = b"info_dict_chunk_here";
        let msg = create_metadata_data_response(1, 0, piece, piece.len());
        assert!(
            msg.payload.ends_with(piece),
            "raw piece bytes must follow the bencode dict"
        );
    }

    #[test]
    fn create_metadata_data_response_encodes_correct_fields() {
        let piece = b"x";
        let msg = create_metadata_data_response(1, 3, piece, 200);
        let body = &msg.payload[1..]; // skip the ut_metadata ID byte
        let bencode_end =
            find_bencode_end(body).expect("bencode end not found in data response payload");
        let parsed = parse_owned(&body[..bencode_end]).expect("invalid bencode in data response");
        let dict = match parsed.first() {
            Some(ValueOwned::Dictionary { entries, .. }) => entries,
            _ => panic!("expected a bencode dictionary"),
        };
        match dict.get(b"msg_type" as &[u8]) {
            Some(ValueOwned::Integer(v)) => assert_eq!(*v, 1, "msg_type must be 1 (data)"),
            _ => panic!("missing or wrong msg_type"),
        }
        match dict.get(b"piece" as &[u8]) {
            Some(ValueOwned::Integer(v)) => assert_eq!(*v, 3),
            _ => panic!("missing or wrong piece"),
        }
        match dict.get(b"total_size" as &[u8]) {
            Some(ValueOwned::Integer(v)) => assert_eq!(*v, 200),
            _ => panic!("missing or wrong total_size"),
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
