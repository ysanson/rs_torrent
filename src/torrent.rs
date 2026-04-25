use std::fs;

use hex::FromHex;
use rustc_hash::FxHashMap;
use url::Url;

use crate::bencode_parser::parser::{ValueOwned, parse_owned};

#[derive(Debug, Clone)]
pub struct Torrent {
    pub announce: String,
    pub creation_date: u32,
    pub length: u32,
    pub piece_length: u32,
    pub total_size: u64,
    pub name: String,
    pub pieces: Vec<[u8; 20]>,
    pub infohash: [u8; 20],
}

#[derive(Debug, Clone)]
pub struct Magnet {
    pub infohash: [u8; 20],
    pub display_name: Option<String>,
    pub trackers: Vec<String>,
    pub file_size: Option<u64>,
}

fn get_string(
    dict: &FxHashMap<Vec<u8>, ValueOwned>,
    key: &[u8],
) -> Result<String, Box<dyn std::error::Error>> {
    match dict.get(key).ok_or("Missing field")? {
        ValueOwned::Bytes(b) => Ok(std::str::from_utf8(b)?.to_string()),
        _ => Err("Expected string field".into()),
    }
}

fn get_u32(
    dict: &FxHashMap<Vec<u8>, ValueOwned>,
    key: &[u8],
) -> Result<u32, Box<dyn std::error::Error>> {
    match dict.get(key).ok_or("Missing field")? {
        ValueOwned::Integer(i) => Ok(*i as u32),
        _ => Err("Expected integer field".into()),
    }
}

fn get_u64(
    dict: &FxHashMap<Vec<u8>, ValueOwned>,
    key: &[u8],
) -> Result<u64, Box<dyn std::error::Error>> {
    match dict.get(key).ok_or("Missing field")? {
        ValueOwned::Integer(i) => Ok(*i as u64),
        _ => Err("Expected integer field".into()),
    }
}
/// Parse torrent from file path
pub fn parse_torrent_file(file_path: &str) -> Result<Torrent, Box<dyn std::error::Error>> {
    let data = fs::read(file_path)?;
    parse_torrent_bytes(&data)
}

/// Parse torrent from byte slice
pub fn parse_torrent_bytes(data: &[u8]) -> Result<Torrent, Box<dyn std::error::Error>> {
    let parsed = parse_owned(data)?;
    let top_level = parsed.first().ok_or("Empty torrent file")?;

    let dict = match top_level {
        ValueOwned::Dictionary { entries, hash: _ } => entries,
        _ => return Err("Invalid torrent file: expected top-level dictionary".into()),
    };

    let announce = get_string(dict, b"announce")?;
    let (info, infohash) = match dict.get(b"info" as &[u8]).ok_or("Missing 'info' field")? {
        ValueOwned::Dictionary { entries, hash } => (entries, hash),
        _ => return Err("Invalid 'info' field".into()),
    };

    let name = get_string(info, b"name")?;
    let piece_length = get_u32(info, b"piece length")?;
    let length = get_u32(info, b"length")?;
    let creation_date = get_u32(dict, b"creation date")?;

    let pieces_bytes = match info
        .get(b"pieces" as &[u8])
        .ok_or("Missing 'pieces' field")?
    {
        ValueOwned::Bytes(b) => b,
        _ => return Err("Invalid 'pieces' field".into()),
    };

    let pieces: Vec<[u8; 20]> = pieces_bytes
        .chunks_exact(20)
        .map(|chunk| <[u8; 20]>::try_from(chunk).unwrap())
        .collect();

    let total_size = if let Ok(length) = get_u64(info, b"length") {
        length
    } else if let Some(ValueOwned::List(files)) = info.get(b"files" as &[u8]) {
        files.iter().fold(0u64, |acc, file| {
            if let ValueOwned::Dictionary { entries, hash: _ } = file {
                if let Ok(length) = get_u64(entries, b"length") {
                    acc + length
                } else {
                    acc
                }
            } else {
                acc
            }
        })
    } else {
        return Err("Cannot determine torrent size".into());
    };

    Ok(Torrent {
        announce,
        creation_date,
        length,
        piece_length,
        total_size,
        name,
        pieces,
        infohash: *infohash,
    })
}

/// Parse a Torrent from a raw bencoded info-dict (as fetched via BEP 9 metadata exchange).
/// The infohash and announce URL must be supplied externally (e.g. from the magnet link).
pub fn parse_info_dict_bytes(
    data: &[u8],
    infohash: [u8; 20],
    announce: String,
) -> Result<Torrent, Box<dyn std::error::Error>> {
    let parsed = parse_owned(data)?;
    let info = match parsed.first().ok_or("Empty info dict")? {
        ValueOwned::Dictionary { entries, .. } => entries,
        _ => return Err("Expected info dictionary".into()),
    };

    let name = get_string(info, b"name")?;
    let piece_length = get_u32(info, b"piece length")?;

    let pieces_bytes = match info.get(b"pieces" as &[u8]).ok_or("Missing 'pieces' field")? {
        ValueOwned::Bytes(b) => b,
        _ => return Err("Invalid 'pieces' field".into()),
    };

    let pieces: Vec<[u8; 20]> = pieces_bytes
        .chunks_exact(20)
        .map(|chunk| <[u8; 20]>::try_from(chunk).unwrap())
        .collect();

    let total_size = if let Ok(length) = get_u64(info, b"length") {
        length
    } else if let Some(ValueOwned::List(files)) = info.get(b"files" as &[u8]) {
        files.iter().fold(0u64, |acc, file| {
            if let ValueOwned::Dictionary { entries, hash: _ } = file {
                if let Ok(length) = get_u64(entries, b"length") {
                    acc + length
                } else {
                    acc
                }
            } else {
                acc
            }
        })
    } else {
        return Err("Cannot determine torrent size".into());
    };

    Ok(Torrent {
        announce,
        creation_date: 0,
        length: total_size as u32,
        piece_length,
        total_size,
        name,
        pieces,
        infohash,
    })
}

pub fn parse_magnet_link(magnet: &str) -> Result<Magnet, Box<dyn std::error::Error>> {
    let url = Url::parse(magnet)?;
    let mut magnet = Magnet {
        infohash: [0u8; 20],
        display_name: None,
        trackers: Vec::new(),
        file_size: None,
    };

    // Process query parameters directly
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "xt" => {
                if let Some(hex) = value.strip_prefix("urn:btih:") {
                    if let Ok(infohash) = <[u8; 20]>::from_hex(hex) {
                        magnet.infohash = infohash;
                    };
                }
            }
            "tr" => {
                magnet.trackers.push(value.into_owned());
            }
            "dn" => {
                magnet.display_name = Some(value.into_owned());
            }
            "xl" => {
                if let Ok(size) = value.parse() {
                    magnet.file_size = Some(size);
                }
            }
            _ => {}
        }
    }

    Ok(magnet)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_magnet_link_valid() {
        let magnet_link = "magnet:?xt=urn:btih:1234567890abcdef1234567890abcdef12345678&dn=Example+Torrent&tr=http://tracker.example.com/announce&xl=1024";
        let result = parse_magnet_link(magnet_link);
        assert!(result.is_ok());

        let magnet = result.unwrap();
        assert_eq!(
            magnet.infohash,
            [
                0x12, 0x34, 0x56, 0x78, 0x90, 0xab, 0xcd, 0xef, 0x12, 0x34, 0x56, 0x78, 0x90, 0xab,
                0xcd, 0xef, 0x12, 0x34, 0x56, 0x78
            ]
        );
        assert_eq!(magnet.display_name, Some("Example Torrent".to_string()));
        assert_eq!(
            magnet.trackers,
            vec!["http://tracker.example.com/announce".to_string()]
        );
        assert_eq!(magnet.file_size, Some(1024));
    }

    #[test]
    fn test_parse_magnet_link_missing_xt() {
        let magnet_link =
            "magnet:?dn=Example+Torrent&tr=http://tracker.example.com/announce&xl=1024";
        let result = parse_magnet_link(magnet_link);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Missing 'xt' parameter or invalid infohash format"
        );
    }

    #[test]
    fn test_parse_magnet_link_invalid_infohash_length() {
        let magnet_link = "magnet:?xt=urn:btih:1234567890abcdef1234567890abcdef1234567&dn=Example+Torrent&tr=http://tracker.example.com/announce&xl=1024";
        let result = parse_magnet_link(magnet_link);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Missing 'xt' parameter or invalid infohash format"
        );
    }

    #[test]
    fn test_parse_magnet_link_missing_optional_params() {
        let magnet_link = "magnet:?xt=urn:btih:1234567890abcdef1234567890abcdef12345678";
        let result = parse_magnet_link(magnet_link);
        assert!(result.is_ok());

        let magnet = result.unwrap();
        assert_eq!(
            magnet.infohash,
            [
                0x12, 0x34, 0x56, 0x78, 0x90, 0xab, 0xcd, 0xef, 0x12, 0x34, 0x56, 0x78, 0x90, 0xab,
                0xcd, 0xef, 0x12, 0x34, 0x56, 0x78
            ]
        );
        assert_eq!(magnet.display_name, None);
        assert_eq!(magnet.trackers, Vec::<String>::new());
        assert_eq!(magnet.file_size, None);
    }

    #[test]
    fn test_parse_magnet_link_multiple_trackers() {
        let magnet_link = "magnet:?xt=urn:btih:1234567890abcdef1234567890abcdef12345678&tr=http://tracker1.example.com/announce&tr=http://tracker2.example.com/announce";
        let result = parse_magnet_link(magnet_link);
        assert!(result.is_ok());

        let magnet = result.unwrap();
        assert_eq!(
            magnet.trackers,
            vec![
                "http://tracker1.example.com/announce".to_string(),
                "http://tracker2.example.com/announce".to_string(),
            ]
        );
    }

    #[test]
    fn test_parse_magnet_link_invalid_file_size() {
        let magnet_link =
            "magnet:?xt=urn:btih:1234567890abcdef1234567890abcdef12345678&xl=invalid_size";
        let result = parse_magnet_link(magnet_link);
        assert!(result.is_ok());

        let magnet = result.unwrap();
        assert_eq!(magnet.file_size, None);
    }
}
