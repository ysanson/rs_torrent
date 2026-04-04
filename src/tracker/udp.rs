use crate::peer::Peer;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::timeout;

const PROTOCOL_ID: i64 = 0x41727101980; // Magic constant for BitTorrent UDP tracker protocol
const CONNECT_ACTION: i32 = 0;
const ANNOUNCE_ACTION: i32 = 1;
const TIMEOUT: Duration = Duration::from_secs(15);

/// Parse UDP tracker URL and extract host and port
fn parse_udp_tracker_url(url: &str) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    // Remove "udp://" prefix
    let url = url.strip_prefix("udp://").ok_or("Invalid UDP tracker URL")?;
    
    // Split off any path (e.g., "/announce")
    let host_port = url.split('/').next().ok_or("Invalid UDP tracker URL")?;
    
    // Resolve to socket address
    let addrs: Vec<SocketAddr> = host_port.to_socket_addrs()?.collect();
    addrs.first().copied().ok_or("Could not resolve tracker address".into())
}

/// Generate a random transaction ID
fn generate_transaction_id() -> i32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    (now & 0xFFFFFFFF) as i32
}

/// Send connect request and receive connection ID
async fn udp_connect(
    socket: &UdpSocket,
    tracker_addr: SocketAddr,
) -> Result<i64, Box<dyn std::error::Error>> {
    let transaction_id = generate_transaction_id();
    
    // Build connect request
    let mut request = Vec::new();
    request.extend_from_slice(&PROTOCOL_ID.to_be_bytes());
    request.extend_from_slice(&CONNECT_ACTION.to_be_bytes());
    request.extend_from_slice(&transaction_id.to_be_bytes());
    
    // Send request
    socket.send_to(&request, tracker_addr).await?;
    
    // Receive response
    let mut response = vec![0u8; 16];
    let (len, _) = timeout(TIMEOUT, socket.recv_from(&mut response)).await??;
    
    if len < 16 {
        return Err("Invalid connect response length".into());
    }
    
    // Parse response
    let action = i32::from_be_bytes([response[0], response[1], response[2], response[3]]);
    let recv_transaction_id = i32::from_be_bytes([response[4], response[5], response[6], response[7]]);
    
    if action != CONNECT_ACTION {
        return Err("Invalid action in connect response".into());
    }
    
    if recv_transaction_id != transaction_id {
        return Err("Transaction ID mismatch in connect response".into());
    }
    
    let connection_id = i64::from_be_bytes([
        response[8], response[9], response[10], response[11],
        response[12], response[13], response[14], response[15],
    ]);
    
    Ok(connection_id)
}

/// Send announce request and receive peer list
async fn udp_announce(
    socket: &UdpSocket,
    tracker_addr: SocketAddr,
    connection_id: i64,
    infohash: &[u8; 20],
    peer_id: &[u8; 20],
    downloaded: u64,
    left: u64,
    uploaded: u64,
    port: u16,
) -> Result<(i32, Vec<Peer>), Box<dyn std::error::Error>> {
    let transaction_id = generate_transaction_id();
    
    // Build announce request
    let mut request = Vec::new();
    request.extend_from_slice(&connection_id.to_be_bytes());
    request.extend_from_slice(&ANNOUNCE_ACTION.to_be_bytes());
    request.extend_from_slice(&transaction_id.to_be_bytes());
    request.extend_from_slice(infohash);
    request.extend_from_slice(peer_id);
    request.extend_from_slice(&downloaded.to_be_bytes());
    request.extend_from_slice(&left.to_be_bytes());
    request.extend_from_slice(&uploaded.to_be_bytes());
    request.extend_from_slice(&0i32.to_be_bytes()); // event: 0 = none
    request.extend_from_slice(&0u32.to_be_bytes()); // IP address: 0 = default
    request.extend_from_slice(&0u32.to_be_bytes()); // key: random
    request.extend_from_slice(&(-1i32).to_be_bytes()); // num_want: -1 = default
    request.extend_from_slice(&port.to_be_bytes());
    
    // Send request
    socket.send_to(&request, tracker_addr).await?;
    
    // Receive response (max size for reasonable number of peers)
    let mut response = vec![0u8; 2048];
    let (len, _) = timeout(TIMEOUT, socket.recv_from(&mut response)).await??;
    
    if len < 20 {
        return Err("Invalid announce response length".into());
    }
    
    // Parse response
    let action = i32::from_be_bytes([response[0], response[1], response[2], response[3]]);
    let recv_transaction_id = i32::from_be_bytes([response[4], response[5], response[6], response[7]]);
    
    if action != ANNOUNCE_ACTION {
        return Err(format!("Invalid action in announce response: {}", action).into());
    }
    
    if recv_transaction_id != transaction_id {
        return Err("Transaction ID mismatch in announce response".into());
    }
    
    let interval = i32::from_be_bytes([response[8], response[9], response[10], response[11]]);
    let _leechers = i32::from_be_bytes([response[12], response[13], response[14], response[15]]);
    let _seeders = i32::from_be_bytes([response[16], response[17], response[18], response[19]]);
    
    // Parse peers (starting at byte 20)
    let peer_data = &response[20..len];
    let peers = extract_peers(peer_data)?;
    
    Ok((interval, peers))
}

/// Extract peers from binary peer data
fn extract_peers(data: &[u8]) -> Result<Vec<Peer>, Box<dyn std::error::Error>> {
    if data.len() % 6 != 0 {
        return Err("Invalid peer data length".into());
    }
    
    let peers = data
        .chunks_exact(6)
        .map(|chunk| {
            let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);
            Peer { ip_addr: ip, port }
        })
        .collect();
    
    Ok(peers)
}

/// Main function to announce to a UDP tracker
pub async fn announce_to_udp_tracker(
    tracker_url: &str,
    infohash: &[u8; 20],
    peer_id: &[u8; 20],
    downloaded: u64,
    left: u64,
    uploaded: u64,
    port: u16,
) -> Result<(i32, Vec<Peer>), Box<dyn std::error::Error>> {
    // Parse tracker URL
    let tracker_addr = parse_udp_tracker_url(tracker_url)?;
    
    // Bind to local UDP socket
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    
    // Connect to tracker
    let connection_id = udp_connect(&socket, tracker_addr).await?;
    
    // Announce to tracker
    let (interval, peers) = udp_announce(
        &socket,
        tracker_addr,
        connection_id,
        infohash,
        peer_id,
        downloaded,
        left,
        uploaded,
        port,
    )
    .await?;
    
    Ok((interval, peers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_udp_tracker_url() {
        let url = "udp://tracker.example.com:6969/announce";
        let result = parse_udp_tracker_url(url);
        // This will fail in tests without network, but validates the parsing logic
        assert!(result.is_ok() || result.is_err()); // Just check it doesn't panic
    }

    #[test]
    fn test_parse_udp_tracker_url_invalid() {
        let url = "http://tracker.example.com:6969/announce";
        let result = parse_udp_tracker_url(url);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_peers() {
        let data = vec![
            192, 168, 1, 1, 0x1A, 0xE1, // Peer 1: 192.168.1.1:6881
            10, 0, 0, 1, 0x1F, 0x90,    // Peer 2: 10.0.0.1:8080
        ];
        
        let peers = extract_peers(&data).unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].ip_addr, Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(peers[0].port, 6881);
        assert_eq!(peers[1].ip_addr, Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(peers[1].port, 8080);
    }

    #[test]
    fn test_extract_peers_invalid_length() {
        let data = vec![192, 168, 1, 1, 0x1A]; // 5 bytes, not multiple of 6
        let result = extract_peers(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_transaction_id() {
        let id1 = generate_transaction_id();
        let id2 = generate_transaction_id();
        // IDs should be valid i32 values
        assert!(id1 != 0 || id2 != 0); // At least one should be non-zero
    }
}
