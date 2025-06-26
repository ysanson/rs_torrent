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
