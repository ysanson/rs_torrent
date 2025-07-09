pub mod bencode_parser;
pub mod peer;
pub mod torrent;
pub mod tracker;

// Re-export commonly used types and functions for easier access
pub use bencode_parser::parser::{Value, ValueOwned, parse, parse_owned};
pub use torrent::{Torrent, parse_torrent_bytes, parse_torrent_file};

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
