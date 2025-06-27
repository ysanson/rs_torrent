pub mod bencode_parser;
use std::fs;

use crate::bencode_parser::{parser::Value, parser::parse};

struct Torrent {
    announce: String,
    creation_date: u32,
    length: u32,
    piece_length: u32,
    name: String,
    pieces: Vec<[u8; 20]>,
    infohash: String,
}

fn extract_torrent(file_path: String) -> Result<Torrent, Box<dyn std::error::Error>> {
    let contents = fs::read(file_path)?.as_slice();
    let binding = parse(contents)?;
    let parsed = binding.first().unwrap();

    if let Value::Dictionary(dict) = parsed {
        // Extract data by copying strings instead of borrowing
        let announce = if let Some(Value::Bytes(data)) = dict.get(b"announce" as &[u8]) {
            String::from_utf8(data.to_vec()).unwrap_or_default()
        } else {
            String::new()
        };

        // Extract other fields similarly...
        let torrent = Torrent {
            announce,
            creation_date: 0,
            length: 0,
            piece_length: 0,
            name: "".to_string(),
            pieces: Vec::new(),
            infohash: "".to_string(),
        };
        Ok(torrent)
    } else {
        Err("Invalid torrent file format".into())
    }
}

fn try_parse() {
    let data = bencode_parser::parser::parse(b"d3:cow3:moo4:spam4:eggse").unwrap();
    let v = data.first().unwrap();

    if let Value::Dictionary(dict) = v {
        let v = dict.get(b"cow" as &[u8]).unwrap();

        if let Value::Bytes(data) = v {
            assert_eq!(data, b"moo");
        }

        let v = dict.get(b"spam" as &[u8]).unwrap();
        if let Value::Bytes(data) = v {
            assert_eq!(data, b"eggs");
        }
    }
}
