use nom::error::{ErrorKind, ParseError};
use std::{fmt::Debug, num::ParseIntError};

#[derive(Debug, thiserror::Error)]
pub enum BencodeError<I> {
    /// A error from a nom parser.
    #[error("a nom error: {1:?}")]
    Nom(I, ErrorKind),
    /// A integer has an invalid form, e.g -0.
    #[error("invalid integer: {0:?}")]
    InvalidInteger(I),
    /// A byte array length is invalid..
    #[error("invalid bytes length: {0:?}")]
    InvalidBytesLength(I),
    /// A integer could not be parsed correctly.
    #[error("parse int error: {0:?}")]
    ParseIntError(I, ParseIntError),
}

impl<I> ParseError<I> for BencodeError<I> {
    fn from_error_kind(input: I, kind: nom::error::ErrorKind) -> Self {
        Self::Nom(input, kind)
    }

    fn append(_: I, _: nom::error::ErrorKind, other: Self) -> Self {
        other
    }
}

impl<I> From<BencodeError<I>> for nom::Err<BencodeError<I>> {
    fn from(value: BencodeError<I>) -> Self {
        match value {
            value @ BencodeError::Nom(_, _) => Self::Error(value),
            value => Self::Failure(value),
        }
    }
}

impl BencodeError<&[u8]> {
    pub fn to_owned(self) -> BencodeError<Vec<u8>> {
        match self {
            BencodeError::Nom(input, kind) => BencodeError::Nom(input.to_vec(), kind),
            BencodeError::InvalidInteger(input) => BencodeError::InvalidInteger(input.to_vec()),
            BencodeError::InvalidBytesLength(input) => {
                BencodeError::InvalidBytesLength(input.to_vec())
            }
            BencodeError::ParseIntError(input, err) => {
                BencodeError::ParseIntError(input.to_vec(), err)
            }
        }
    }
}
