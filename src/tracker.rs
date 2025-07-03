use crate::Torrent;
use percent_encoding::{NON_ALPHANUMERIC, percent_encode};
use url::{ParseError, Url};

const PEER_ID: [u8; 20] = *b"f52c3727bfe8600e8923";

pub fn build_tracker_url(torrent: &Torrent, port: u16) -> Result<String, ParseError> {
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
