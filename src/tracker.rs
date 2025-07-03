use crate::Torrent;
use url::{Url} ;

const PEER_ID: [u8; 20] = *b"f52c3727bfe8600e8923";

pub fn buld_tracker_url(
    torrent: &Torrent,
    port: u16,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut url = Url::parse(&torrent.announce)?;
    "".to_string()
}
