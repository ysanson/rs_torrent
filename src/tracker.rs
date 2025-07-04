use std::{error::Error, net::IpAddr};

use crate::{Torrent, ValueOwned, parse_owned};
use once_cell::sync::Lazy;
use percent_encoding::{NON_ALPHANUMERIC, percent_encode};
use reqwest::Client;
use url::{ParseError, Url};

const PEER_ID: [u8; 20] = *b"f52c3727bfe8600e8923";

const HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .user_agent("rs_torrent/0.1")
        .build()
        .expect("Failed to build HTTP client")
});

pub struct Peer {
    pub ip_addr: IpAddr,
    pub port: u16,
}

fn build_tracker_url(torrent: &Torrent, port: u16) -> Result<String, ParseError> {
    let mut url = Url::parse(&torrent.announce)?;
    let encoded_infohash = percent_encode(&torrent.infohash, NON_ALPHANUMERIC).to_string();
    let encoded_peer_id = percent_encode(&PEER_ID, NON_ALPHANUMERIC).to_string();
    url.query_pairs_mut()
        .append_pair("info_hash", &encoded_infohash)
        .append_pair("peer_id", &encoded_peer_id)
        .append_pair("port", &port.to_string())
        .append_pair("uploaded", "0")
        .append_pair("downloaded", "0")
        .append_pair("compact", "1")
        .append_pair("left", &torrent.length.to_string());
    Ok(url.to_string())
}

async fn contact_tracker(tracker_url: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let response = HTTP_CLIENT.get(tracker_url).send().await?;

    if !response.status().is_success() {
        return Err(format!("Tracker error: {}", response.status()).into());
    }

    let bytes = response.bytes().await?;
    Ok(bytes.to_vec())
}

fn extract_peers(bytes: &Vec<u8>) -> Option<Vec<Peer>> {
    return None;
}

pub async fn announce_to_tracker(torrent: &Torrent, port: u16) -> Result<(), Box<dyn Error>> {
    let url = build_tracker_url(torrent, port)?;
    let response_bytes = contact_tracker(&url).await?;

    let values = parse_owned(&response_bytes)?;
    let dict = match values.first() {
        Some(ValueOwned::Dictionary { entries, hash: _ }) => entries,
        _ => return Err("Tracker response not a dict".into()),
    };

    if let Some(ValueOwned::Integer(interval)) = dict.get(b"interval" as &[u8]) {
        println!("Tracker reannounce interval: {interval} seconds");
    }

    let peers = match dict.get(b"peers" as &[u8]) {
        Some(ValueOwned::Bytes(peers)) => extract_peers(peers),
        _ => return Err("Peers is not a dict".into()),
    };

    Ok(())
}
