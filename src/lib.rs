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

async fn announce_completion_to_tracker(
    torrent: &Torrent,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = crate::tracker::build_completion_tracker_url(torrent, port)?;
    let _response = crate::tracker::contact_tracker(&url).await?;
    println!("📢 Sent completion event to tracker");
    Ok(())
}

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
    let (reannounce_interval, initial_peers) = announce_to_tracker(&torrent, 6881).await?;

    println!(
        "Initial tracker response: {} peers, reannounce interval: {}s",
        initial_peers.len(),
        reannounce_interval
    );

    // Connect to initial peers without waiting
    client.connect_to_new_peers(initial_peers).await;

    // Start tracker reannouncement task
    let client_reannounce = client.clone();
    let torrent_clone = torrent.clone();
    let reannounce_handle = tokio::spawn(async move {
        let mut interval = reannounce_interval as u64;

        loop {
            sleep(Duration::from_secs(interval)).await;

            // Check if download is complete before reannouncing
            if client_reannounce.is_complete().await {
                println!("📢 Download complete, stopping tracker announcements");
                break;
            }

            println!("📢 Reannouncing to tracker...");

            // Handle tracker announcement in a scoped block to avoid Send issues
            let (new_peers_to_connect, new_interval_opt) = {
                match announce_to_tracker(&torrent_clone, 6881).await {
                    Ok((new_interval, new_peers)) => {
                        if new_peers.is_empty() {
                            println!(
                                "📢 Tracker reannounce: no new peers, next in {}s",
                                new_interval
                            );
                        } else {
                            println!(
                                "📢 Tracker reannounce: {} new peers discovered, next in {}s",
                                new_peers.len(),
                                new_interval
                            );
                        }
                        (Some(new_peers), Some(new_interval as u64))
                    }
                    Err(e) => {
                        eprintln!(
                            "Tracker reannounce failed: {}, retrying in {}s",
                            e, interval
                        );
                        (None, None)
                    }
                }
            };

            // Update interval if we got a new one
            if let Some(new_interval) = new_interval_opt {
                interval = new_interval;
            }

            // Connect to new peers if we got any
            if let Some(new_peers) = new_peers_to_connect {
                client_reannounce.connect_to_new_peers(new_peers).await;
            }
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
        let total_peers = peer_info.len();
        let total_pending: usize = peer_info.iter().map(|(_, pending, _)| pending).sum();
        println!(
            "Peers: {}/{} active, {} total pending requests",
            active_peers, total_peers, total_pending
        );

        if client.is_complete().await {
            println!("\n✅ Download complete!");

            // Announce completion to tracker
            println!("📢 Announcing completion to tracker...");
            match announce_completion_to_tracker(&torrent, 6881).await {
                Ok(_) => println!("✅ Completion announced to tracker"),
                Err(e) => eprintln!("Failed to announce completion: {}", e),
            }

            break;
        }

        sleep(Duration::from_secs(2)).await;
    }

    // Cancel the reannouncement task since download is complete
    reannounce_handle.abort();

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
