/// Big thanks to https://github.com/edg-l/nom-bencode for this tutorial!!
use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::take,
    character::complete::{char, digit1},
    combinator::{eof, recognize},
    multi::{many_till, many0},
    sequence::{delimited, pair, preceded},
};
use std::{collections::HashMap, fmt::Debug};

pub use nom::Err;

use crate::bencode_parser::errors::BencodeError;
type BenResult<'a> = IResult<&'a [u8], Value<'a>, BencodeError<&'a [u8]>>;

#[derive(Debug, Clone)]
pub enum Value<'a> {
    Bytes(&'a [u8]),
    Integer(i64),
    List(Vec<Self>),
    Dictionary(HashMap<&'a [u8], Self>),
}

impl<'a> Value<'a> {
    fn parse_integer(start_inp: &'a [u8]) -> BenResult<'a> {
        let (inp, value) = delimited(
            char('i'),
            alt((
                recognize(pair(char('+'), digit1)),
                recognize(pair(char('-'), digit1)),
                digit1,
            )),
            char('e'),
        )
        .parse(start_inp)?;

        let value_str =
            std::str::from_utf8(value).expect("value should be a valid integer str at this point");

        if value_str.starts_with("-0") || (value_str.starts_with('0') && value_str.len() > 1) {
            Err(nom::Err::Failure(BencodeError::InvalidInteger(start_inp)))
        } else {
            let value_integer: i64 = value_str
                .parse()
                .map_err(|e| BencodeError::ParseIntError(inp, e))?;
            Ok((inp, Value::Integer(value_integer)))
        }
    }

    fn parse_bytes(start_inp: &'a [u8]) -> BenResult<'a> {
        let (inp, length) = digit1(start_inp)?;

        let (inp, _) = char(':')(inp)?;

        let length = std::str::from_utf8(length)
            .expect("length should be a valid integer str at this point");

        let length: u64 = length
            .parse()
            .map_err(|e| BencodeError::ParseIntError(inp, e))?;

        if length == 0 {
            Err(BencodeError::InvalidBytesLength(start_inp))?;
        }

        let (inp, characters) = take(length)(inp)?;

        Ok((inp, Value::Bytes(characters)))
    }

    fn parse_list(start_inp: &'a [u8]) -> BenResult<'a> {
        let (inp, value) = preceded(
            char('l'),
            many_till(
                alt((
                    Self::parse_bytes,
                    Self::parse_integer,
                    Self::parse_list,
                    Self::parse_dict,
                )),
                char('e'),
            ),
        )
        .parse(start_inp)?;

        Ok((inp, Value::List(value.0)))
    }

    fn parse_dict(start_inp: &'a [u8]) -> BenResult<'a> {
        let (inp, value) = preceded(
            char('d'),
            many_till(
                pair(
                    Self::parse_bytes,
                    alt((
                        Self::parse_bytes,
                        Self::parse_integer,
                        Self::parse_list,
                        Self::parse_dict,
                    )),
                ),
                char('e'),
            ),
        )
        .parse(start_inp)?;

        let data = value.0.into_iter().map(|x| {
            // Keys are always string
            if let Value::Bytes(key) = x.0 {
                (key, x.1)
            } else {
                unreachable!()
            }
        });

        let map = data.collect();

        Ok((inp, Value::Dictionary(map)))
    }
}

#[derive(Debug, Clone)]
pub enum ValueOwned {
    Bytes(Vec<u8>),
    Integer(i64),
    List(Vec<ValueOwned>),
    Dictionary(HashMap<Vec<u8>, ValueOwned>),
}

impl<'a> From<Value<'a>> for ValueOwned {
    fn from(v: Value<'a>) -> Self {
        match v {
            Value::Bytes(b) => ValueOwned::Bytes(b.to_vec()),
            Value::Integer(i) => ValueOwned::Integer(i),
            Value::List(lst) => ValueOwned::List(lst.into_iter().map(ValueOwned::from).collect()),
            Value::Dictionary(dict) => ValueOwned::Dictionary(
                dict.into_iter()
                    .map(|(k, v)| (k.to_vec(), ValueOwned::from(v)))
                    .collect(),
            ),
        }
    }
}

/// Parses the provided bencode `source`.
///
/// # Errors
/// Returns `Err` if there was an error parsing `source`.
pub fn parse(source: &[u8]) -> Result<Vec<Value>, Err<BencodeError<&[u8]>>> {
    let (source2, items) = many0(alt((
        Value::parse_bytes,
        Value::parse_integer,
        Value::parse_list,
        Value::parse_dict,
    )))
    .parse(source)?;

    let _ = eof(source2)?;

    Ok(items)
}

pub fn parse_owned(source: &[u8]) -> Result<Vec<ValueOwned>, Err<BencodeError<Vec<u8>>>> {
    let items = parse(source).map_err(|e| e.map(|err| err.to_owned()))?;

    Ok(items.into_iter().map(ValueOwned::from).collect())
}

#[cfg(test)]
mod tests {
    use crate::bencode_parser::{
        errors::BencodeError,
        parser::{Value, parse},
    };
    use assert_matches::assert_matches;

    #[test]
    fn test_integer() {
        let (_, v) = Value::parse_integer(b"i3e").unwrap();
        assert_matches!(v, Value::Integer(3));

        let (_, v) = Value::parse_integer(b"i3e1:a").unwrap();
        assert_matches!(v, Value::Integer(3));

        let (_, v) = Value::parse_integer(b"i-3e").unwrap();
        assert_matches!(v, Value::Integer(-3));

        let (_, v) = Value::parse_integer(b"i333333e").unwrap();
        assert_matches!(v, Value::Integer(333_333));

        let v = Value::parse_integer(b"i-0e").unwrap_err();
        assert_matches!(v, nom::Err::Failure(BencodeError::InvalidInteger(_)));

        let v = Value::parse_integer(b"i00e").unwrap_err();
        assert_matches!(v, nom::Err::Failure(BencodeError::InvalidInteger(_)));

        let v = Value::parse_integer(b"i-00e").unwrap_err();
        assert_matches!(v, nom::Err::Failure(BencodeError::InvalidInteger(_)));

        let v = Value::parse_integer(b"i03e").unwrap_err();
        assert_matches!(v, nom::Err::Failure(BencodeError::InvalidInteger(_)));

        let v = Value::parse_integer(b"i0040e").unwrap_err();
        assert_matches!(v, nom::Err::Failure(BencodeError::InvalidInteger(_)));

        let v = Value::parse_integer(b"li3ee").unwrap_err();
        assert_matches!(v, nom::Err::Error(BencodeError::Nom(..)));
    }

    #[test]
    fn test_string() {
        let (_, v) = Value::parse_bytes(b"4:abcd").unwrap();
        assert_matches!(v, Value::Bytes(b"abcd"));

        let (_, v) = Value::parse_bytes(b"1:a").unwrap();
        assert_matches!(v, Value::Bytes(b"a"));

        let (_, v) = Value::parse_bytes(b"1:rock").unwrap();
        assert_matches!(v, Value::Bytes(b"r"));

        let v = Value::parse_bytes(b"0:a").unwrap_err();
        assert_matches!(v, nom::Err::Failure(BencodeError::InvalidBytesLength(_)));
    }

    #[test]
    fn test_list() {
        let (_, v) = Value::parse_list(b"l4:spam4:eggsi22eli1ei2eee").unwrap();
        assert_matches!(v, Value::List(_));

        if let Value::List(list) = v {
            let mut it = list.iter();

            let x = it.next().unwrap();
            assert_matches!(*x, Value::Bytes(b"spam"));

            let x = it.next().unwrap();
            assert_matches!(*x, Value::Bytes(b"eggs"));

            let x = it.next().unwrap();
            assert_matches!(*x, Value::Integer(22));

            let x = it.next().unwrap();
            assert_matches!(*x, Value::List(_));

            if let Value::List(list) = x {
                let mut it = list.iter();

                let x = it.next().unwrap();
                assert_matches!(*x, Value::Integer(1));

                let x = it.next().unwrap();
                assert_matches!(*x, Value::Integer(2));
            }
        }
    }

    #[test]
    fn test_list_empty() {
        let (_, v) = Value::parse_list(b"le").unwrap();
        assert_matches!(v, Value::List(_));
    }

    #[test]
    fn test_dict() {
        let (_, v) = Value::parse_dict(b"d3:cow3:moo4:spam4:eggse").unwrap();
        assert_matches!(v, Value::Dictionary(_));

        if let Value::Dictionary(dict) = v {
            let v = dict.get(b"cow".as_slice()).unwrap();
            assert_matches!(*v, Value::Bytes(b"moo"));

            let v = dict.get(b"spam".as_slice()).unwrap();
            assert_matches!(*v, Value::Bytes(b"eggs"));
        }

        let (_, v) = Value::parse_dict(b"d4:spaml1:a1:bee").unwrap();
        assert_matches!(v, Value::Dictionary(_));

        if let Value::Dictionary(dict) = v {
            let v = dict.get(b"spam".as_slice()).unwrap();
            assert_matches!(*v, Value::List(_));
        }
    }

    #[test]
    fn test_parse() {
        let data = parse(b"d3:cow3:moo4:spam4:eggse").unwrap();
        let v = data.first().unwrap();
        assert_matches!(v, Value::Dictionary(_));

        if let Value::Dictionary(dict) = v {
            let v = dict.get(b"cow".as_slice()).unwrap();
            assert_matches!(*v, Value::Bytes(b"moo"));

            let v = dict.get(b"spam".as_slice()).unwrap();
            assert_matches!(*v, Value::Bytes(b"eggs"));
        }

        let (_, v) = Value::parse_dict(b"d4:spaml1:a1:bee").unwrap();
        assert_matches!(v, Value::Dictionary(_));

        if let Value::Dictionary(dict) = v {
            let v = dict.get(b"spam".as_slice()).unwrap();
            assert_matches!(*v, Value::List(_));
        }
    }

    #[test]
    fn test_parse_invalid_integer() {
        let data = Value::parse_integer(b"123");
        assert!(data.is_err());
    }

    #[test]
    fn test_parse_invalid_bytes() {
        let data = Value::parse_bytes(b"123");
        assert!(data.is_err());
    }

    #[test]
    fn test_parse_invalid_list() {
        let data = Value::parse_list(b"123");
        assert!(data.is_err());

        let data = Value::parse_list(b"l123");
        assert!(data.is_err());

        let data = Value::parse_list(b"li1e");
        assert!(data.is_err());
    }

    #[test]
    fn test_parse_invalid_dict() {
        let data = Value::parse_dict(b"123");
        assert!(data.is_err());

        let data = Value::parse_dict(b"d123");
        assert!(data.is_err());
    }

    #[test]
    fn test_parse_invalid_x() {
        let data = parse(b"123");
        assert!(data.is_err());
    }

    #[test]
    fn test_parse_torrent() {
        let data = parse(include_bytes!(
            "../../test_data/debian-12.11.0-amd64-netinst.iso.torrent"
        ))
        .unwrap();
        assert_eq!(data.len(), 1);

        let v = data.first().unwrap();
        assert_matches!(*v, Value::Dictionary(_));

        if let Value::Dictionary(dict) = v {
            let info = dict.get(b"info".as_slice()).unwrap();
            assert_matches!(*info, Value::Dictionary(_));

            let announce = dict.get(b"announce".as_slice()).unwrap();
            assert_matches!(*announce, Value::Bytes(_));

            if let Value::Bytes(announce) = *announce {
                let announce = std::str::from_utf8(announce).unwrap();
                assert_eq!(announce, "http://bttracker.debian.org:6969/announce");
            }

            let url_list = dict.get(b"url-list".as_slice()).unwrap();
            assert_matches!(*url_list, Value::List(_));
        }
    }
}
