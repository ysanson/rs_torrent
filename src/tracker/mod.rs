mod http;
mod udp;

pub use http::{PEER_ID, build_completion_tracker_url, contact_tracker};
use http::announce_to_tracker as announce_to_http_tracker;
use udp::announce_to_udp_tracker;

use crate::peer::Peer;
use std::error::Error;

/// Announce to a tracker (automatically detects HTTP/HTTPS or UDP)
pub async fn announce_to_tracker(
    announce_url: &str,
    infohash: &[u8],
    length: &u64,
    port: u16,
) -> Result<(i64, Vec<Peer>), Box<dyn Error>> {
    // Check if this is a UDP tracker
    if announce_url.starts_with("udp://") {
        println!("📡 Using UDP tracker: {}", announce_url);
        match announce_to_udp_tracker(
            announce_url,
            infohash.try_into().map_err(|_| "Invalid infohash length")?,
            &PEER_ID,
            0,       // downloaded
            *length, // left
            0,       // uploaded
            port,
        )
        .await
        {
            Ok((interval, peers)) => {
                println!("✅ UDP tracker response: {} peers, interval: {}s", peers.len(), interval);
                return Ok((interval as i64, peers));
            }
            Err(e) => {
                eprintln!("⚠️ UDP tracker failed: {}", e);
                return Err(e);
            }
        }
    }
    
    // HTTP/HTTPS tracker
    println!("📡 Using HTTP tracker: {}", announce_url);
    announce_to_http_tracker(announce_url, infohash, length, port).await
}
