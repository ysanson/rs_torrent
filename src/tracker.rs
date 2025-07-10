use crate::peer::Peer;
use crate::{Torrent, ValueOwned, parse_owned};
use once_cell::sync::Lazy;
use percent_encoding::{NON_ALPHANUMERIC, percent_encode};
use reqwest::Client;
use std::{error::Error, net::Ipv4Addr};
use url::{ParseError, Url};

pub const PEER_ID: [u8; 20] = *b"f52c3727bfe8600e8923";
const HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .user_agent("rs_torrent/0.1")
        .build()
        .expect("Failed to build HTTP client")
});

fn encode_bytes(bytes: &[u8]) -> String {
    percent_encode(bytes, NON_ALPHANUMERIC).to_string()
}

fn build_tracker_url(torrent: &Torrent, port: u16) -> Result<String, ParseError> {
    let mut base = Url::parse(&torrent.announce)?;

    let info_hash = encode_bytes(&torrent.infohash);
    let peer_id = encode_bytes(&PEER_ID);

    let query = format!(
        "info_hash={}&peer_id={}&port={}&uploaded=0&downloaded=0&compact=1&left={}",
        info_hash, peer_id, port, torrent.length
    );

    base.set_query(Some(&query));
    Ok(base.to_string())
}

async fn contact_tracker(tracker_url: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let response = HTTP_CLIENT.get(tracker_url).send().await?;

    if !response.status().is_success() {
        return Err(format!("Tracker error: {}", response.status()).into());
    }

    let bytes = response.bytes().await?;
    Ok(bytes.to_vec())
}

fn extract_peers(bytes: &[u8]) -> Option<Vec<Peer>> {
    let peer_size: usize = 6;
    if bytes.len() % peer_size != 0 {
        return None;
    }
    let peers = bytes
        .chunks_exact(6)
        .map(|chunk| {
            let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);
            Peer { ip_addr: ip, port }
        })
        .collect();

    Some(peers)
}

pub async fn announce_to_tracker(
    torrent: &Torrent,
    port: u16,
) -> Result<(i64, Vec<Peer>), Box<dyn Error>> {
    let url = build_tracker_url(torrent, port)?;
    let response_bytes = contact_tracker(&url).await?;
    let values = parse_owned(&response_bytes)?;
    let dict = match values.first() {
        Some(ValueOwned::Dictionary { entries, hash: _ }) => entries,
        _ => return Err("Tracker response not a dict".into()),
    };

    let interval = match dict.get(b"interval" as &[u8]) {
        Some(ValueOwned::Integer(interval)) => interval,
        _ => return Err("Failed to extract interval".into()),
    };
    println!("Interval is {:?}", interval);

    let peers = match dict.get(b"peers" as &[u8]) {
        Some(ValueOwned::Bytes(peers)) => extract_peers(peers),
        _ => return Err("Peers is not a dict".into()),
    };

    if peers.is_none() {
        return Err("Peers cannot be extracted".into());
    }

    Ok((*interval, peers.unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::Ipv4Addr;

    fn create_test_torrent() -> Torrent {
        Torrent {
            announce: "http://tracker.example.com:8080/announce".to_string(),
            creation_date: 1234567890,
            length: 1048576,
            piece_length: 16384,
            total_size: 1048576,
            name: "test.txt".to_string(),
            pieces: vec![[0u8; 20]; 64],
            infohash: [
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
            ],
        }
    }

    #[test]
    fn test_build_tracker_url() {
        let torrent = create_test_torrent();
        let port = 6881;
        let url = build_tracker_url(&torrent, port).unwrap();

        assert!(url.contains("http://tracker.example.com:8080/announce"));
        assert!(url.contains("port=6881"));
        assert!(url.contains("uploaded=0"));
        assert!(url.contains("downloaded=0"));
        assert!(url.contains("compact=1"));
        assert!(url.contains("left=1048576"));
        assert!(url.contains("info_hash="));
        assert!(url.contains("peer_id="));
    }

    #[test]
    fn test_build_tracker_url_invalid_announce() {
        let mut torrent = create_test_torrent();
        torrent.announce = "invalid_url".to_string();
        let port = 6881;
        let result = build_tracker_url(&torrent, port);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_tracker_url_encoding() {
        let torrent = create_test_torrent();
        let port = 6881;
        let url = build_tracker_url(&torrent, port).unwrap();

        // Check that infohash is percent-encoded (should contain encoded bytes)
        assert!(url.contains("info_hash="));
        // The actual encoding depends on the NON_ALPHANUMERIC encoding set
        // Just check that it's properly encoded by looking for % symbols
        let info_hash_start = url.find("info_hash=").unwrap() + "info_hash=".len();
        let info_hash_end = url[info_hash_start..]
            .find('&')
            .unwrap_or(url.len() - info_hash_start);
        let info_hash_value = &url[info_hash_start..info_hash_start + info_hash_end];
        assert!(info_hash_value.contains('%'));

        // Check that peer_id is encoded
        assert!(url.contains("peer_id=f52c3727bfe8600e8923"));
    }

    #[test]
    fn test_extract_peers_valid() {
        // Create binary peer data: 2 peers
        // Peer 1: IP 192.168.1.1, Port 6881 (0x1AE1)
        // Peer 2: IP 10.0.0.1, Port 8080 (0x1F90)
        let peer_data = vec![
            192, 168, 1, 1, 0x1A, 0xE1, // Peer 1
            10, 0, 0, 1, 0x1F, 0x90, // Peer 2
        ];

        let peers = extract_peers(&peer_data).unwrap();
        assert_eq!(peers.len(), 2);

        assert_eq!(peers[0].ip_addr, Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(peers[0].port, 6881);

        assert_eq!(peers[1].ip_addr, Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(peers[1].port, 8080);
    }

    #[test]
    fn test_extract_peers_empty() {
        let peer_data = vec![];
        let peers = extract_peers(&peer_data).unwrap();
        assert_eq!(peers.len(), 0);
    }

    #[test]
    fn test_extract_peers_invalid_length() {
        // Invalid length (not multiple of 6)
        let peer_data = vec![192, 168, 1, 1, 0x1A]; // 5 bytes
        let result = extract_peers(&peer_data);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_peers_single() {
        // Single peer: IP 127.0.0.1, Port 9999 (0x270F)
        let peer_data = vec![127, 0, 0, 1, 0x27, 0x0F];
        let peers = extract_peers(&peer_data).unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].ip_addr, Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(peers[0].port, 9999);
    }

    #[test]
    fn test_extract_peers_multiple() {
        // 3 peers
        let peer_data = vec![
            192, 168, 1, 1, 0x1A, 0xE1, // Peer 1
            10, 0, 0, 1, 0x1F, 0x90, // Peer 2
            172, 16, 0, 1, 0x50, 0x00, // Peer 3: 172.16.0.1:20480
        ];

        let peers = extract_peers(&peer_data).unwrap();
        assert_eq!(peers.len(), 3);

        assert_eq!(peers[2].ip_addr, Ipv4Addr::new(172, 16, 0, 1));
        assert_eq!(peers[2].port, 20480);
    }

    #[test]
    fn test_peer_id_constant() {
        assert_eq!(PEER_ID.len(), 20);
        assert_eq!(PEER_ID, *b"f52c3727bfe8600e8923");
    }

    #[test]
    fn test_port_encoding_edge_cases() {
        let torrent = create_test_torrent();

        // Test port 0
        let url = build_tracker_url(&torrent, 0).unwrap();
        assert!(url.contains("port=0"));

        // Test max port
        let url = build_tracker_url(&torrent, 65535).unwrap();
        assert!(url.contains("port=65535"));
    }

    #[test]
    fn test_extract_peers_port_endianness() {
        // Test big-endian port encoding
        // Port 256 should be encoded as [0x01, 0x00]
        let peer_data = vec![192, 168, 1, 1, 0x01, 0x00];
        let peers = extract_peers(&peer_data).unwrap();
        assert_eq!(peers[0].port, 256);

        // Port 1 should be encoded as [0x00, 0x01]
        let peer_data = vec![192, 168, 1, 1, 0x00, 0x01];
        let peers = extract_peers(&peer_data).unwrap();
        assert_eq!(peers[0].port, 1);
    }

    #[test]
    fn test_build_tracker_url_with_query_params() {
        let mut torrent = create_test_torrent();
        torrent.announce = "http://tracker.example.com:8080/announce?existing=param".to_string();
        let port = 6881;
        let url = build_tracker_url(&torrent, port).unwrap();

        // Should preserve existing query params and add new ones
        assert!(url.contains("existing=param"));
        assert!(url.contains("port=6881"));
    }

    #[test]
    fn test_build_tracker_url_different_schemes() {
        let mut torrent = create_test_torrent();

        // Test HTTPS
        torrent.announce = "https://tracker.example.com/announce".to_string();
        let url = build_tracker_url(&torrent, 6881).unwrap();
        assert!(url.starts_with("https://"));

        // Test HTTP
        torrent.announce = "http://tracker.example.com/announce".to_string();
        let url = build_tracker_url(&torrent, 6881).unwrap();
        assert!(url.starts_with("http://"));
    }

    #[test]
    fn test_extract_peers_boundary_ips() {
        // Test boundary IP addresses
        let peer_data = vec![
            0, 0, 0, 0, 0x1A, 0xE1, // 0.0.0.0:6881
            255, 255, 255, 255, 0x1F, 0x90, // 255.255.255.255:8080
        ];

        let peers = extract_peers(&peer_data).unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].ip_addr, Ipv4Addr::new(0, 0, 0, 0));
        assert_eq!(peers[1].ip_addr, Ipv4Addr::new(255, 255, 255, 255));
    }

    #[test]
    fn test_build_tracker_url_length_values() {
        let mut torrent = create_test_torrent();

        // Test with 0 length
        torrent.length = 0;
        let url = build_tracker_url(&torrent, 6881).unwrap();
        assert!(url.contains("left=0"));

        // Test with large length
        torrent.length = u32::MAX;
        let url = build_tracker_url(&torrent, 6881).unwrap();
        assert!(url.contains(&format!("left={}", u32::MAX)));
    }
}
