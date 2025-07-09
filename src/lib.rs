pub mod bencode_parser;
pub mod peer;
pub mod torrent;
pub mod tracker;

use std::time::Duration;

// Re-export commonly used types and functions for easier access
pub use bencode_parser::parser::{Value, ValueOwned, parse, parse_owned};
use tokio::time::sleep;
pub use torrent::{Torrent, parse_torrent_bytes, parse_torrent_file};

use crate::peer::{BitTorrentClient, DownloadState};
use crate::tracker::{PEER_ID, announce_to_tracker};

pub async fn download_from_torrent_file(
    file_path: &str,
    output_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let torrent = parse_torrent_file(&file_path)?;
    let download_state = DownloadState::new(
        torrent.pieces.len(),
        torrent.piece_length,
        torrent.infohash,
        torrent.pieces.clone(),
        torrent.total_size,
    );
    let client = BitTorrentClient::new(download_state, PEER_ID);
    let (_reannounce, peers) = announce_to_tracker(&torrent, 6881).await?;

    // Start download in a separate task
    let client_clone = client.clone();
    let download_handle = tokio::spawn(async move {
        if let Err(e) = client_clone.start_download(peers).await {
            eprintln!("Download error: {}", e);
        }
    });

    // Step 6: Monitor progress
    loop {
        let (downloaded_pieces, total_pieces) = client.get_progress().await;
        let (downloaded_blocks, total_blocks) = client.get_block_progress().await;
        let pipeline_stats = client.get_pipeline_stats().await;
        let peer_info = client.get_peer_info().await;

        let piece_progress = (downloaded_pieces as f64 / total_pieces as f64) * 100.0;
        let block_progress = (downloaded_blocks as f64 / total_blocks as f64) * 100.0;

        println!(
            "Progress: {}/{} pieces ({:.1}%) | {}/{} blocks ({:.1}%)",
            downloaded_pieces,
            total_pieces,
            piece_progress,
            downloaded_blocks,
            total_blocks,
            block_progress
        );

        // Show pipelining statistics
        println!(
            "Pipeline: {} requests sent, {} responses, {} timeouts, max depth: {}",
            pipeline_stats.total_requests_sent,
            pipeline_stats.total_responses_received,
            pipeline_stats.total_timeouts,
            pipeline_stats.max_pipeline_depth_seen
        );

        // Show active peer connections
        let active_peers = peer_info
            .iter()
            .filter(|(_, _, can_download)| *can_download)
            .count();
        let total_pending: usize = peer_info.iter().map(|(_, pending, _)| pending).sum();
        println!(
            "Peers: {} active, {} total pending requests",
            active_peers, total_pending
        );

        if client.is_complete().await {
            println!("\n✅ Download complete!");
            break;
        }

        sleep(Duration::from_secs(2)).await;
    }

    // Wait for download task to complete
    download_handle.await?;
    client.write_to_file(output_path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_bencode() {
        let data = b"d3:cow3:moo4:spam4:eggse";
        let parsed = parse(data).unwrap();

        if let Some(Value::Dictionary {
            entries: dict,
            hash: _,
        }) = parsed.first()
        {
            if let Some(Value::Bytes(cow_value)) = dict.get(b"cow" as &[u8]) {
                assert_eq!(cow_value, b"moo");
            }

            if let Some(Value::Bytes(spam_value)) = dict.get(b"spam" as &[u8]) {
                assert_eq!(spam_value, b"eggs");
            }
        }
    }

    #[test]
    fn test_parse_owned() {
        let data = b"d3:cow3:moo4:spam4:eggse";
        let parsed = parse_owned(data).unwrap();

        if let Some(ValueOwned::Dictionary {
            entries: dict,
            hash: _,
        }) = parsed.first()
        {
            if let Some(ValueOwned::Bytes(cow_value)) = dict.get(b"cow" as &[u8]) {
                assert_eq!(cow_value, b"moo");
            }

            if let Some(ValueOwned::Bytes(spam_value)) = dict.get(b"spam" as &[u8]) {
                assert_eq!(spam_value, b"eggs");
            }
        }
    }
}
