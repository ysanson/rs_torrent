pub mod bencode_parser;
use crate::bencode_parser::parser::Value;

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
