use std::net::Ipv4Addr;

pub mod client;
pub mod handshake;
pub mod message;
pub mod state;

const PSTR: &str = "BitTorrent protocol";
const PSTR_LEN: u8 = PSTR.len() as u8; // always 19

#[derive(Debug, Clone)]
pub struct Peer {
    pub ip_addr: Ipv4Addr,
    pub port: u16,
}

pub use client::{BitTorrentClient, PIECE_BLOCK_SIZE};
pub use state::DownloadState;
