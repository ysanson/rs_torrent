use std::{collections::HashMap, fs};

use crate::bencode_parser::parser::{ValueOwned, parse_owned};

#[derive(Debug, Clone)]
pub struct Torrent {
    pub announce: String,
    pub creation_date: u32,
    pub length: u32,
    pub piece_length: u32,
    pub total_size: u32,
    pub name: String,
    pub pieces: Vec<[u8; 20]>,
    pub infohash: [u8; 20],
}

pub fn get_string(
    dict: &HashMap<Vec<u8>, ValueOwned>,
    key: &[u8],
) -> Result<String, Box<dyn std::error::Error>> {
    match dict.get(key).ok_or("Missing field")? {
        ValueOwned::Bytes(b) => Ok(std::str::from_utf8(b)?.to_string()),
        _ => Err("Expected string field".into()),
    }
}

pub fn get_u32(
    dict: &HashMap<Vec<u8>, ValueOwned>,
    key: &[u8],
) -> Result<u32, Box<dyn std::error::Error>> {
    match dict.get(key).ok_or("Missing field")? {
        ValueOwned::Integer(i) => Ok(*i as u32),
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
    let parsed = parse_owned(&data)?;
    let top_level = parsed.first().ok_or("Empty torrent file")?;

    let dict = match top_level {
        ValueOwned::Dictionary {
            entries: d,
            hash: _,
        } => d,
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

    let total_size = if let Ok(length) = get_u32(info, b"length") {
        length as u32
    } else if let Some(ValueOwned::List(files)) = info.get(b"files" as &[u8]) {
        files.iter().fold(0u32, |acc, file| {
            if let ValueOwned::Dictionary {
                entries: file_dict,
                hash: _,
            } = file
            {
                if let Ok(length) = get_u32(file_dict, b"length") {
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

    // let infohash = compute_info_hash(info);

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
