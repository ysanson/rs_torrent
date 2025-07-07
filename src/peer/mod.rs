use std::net::Ipv4Addr;

pub mod handshake;
pub mod message;

const PSTR: &str = "BitTorrent protocol";
const PSTR_LEN: u8 = PSTR.len() as u8; // always 19

#[derive(Debug)]
pub struct Peer {
    pub ip_addr: Ipv4Addr,
    pub port: u16,
}
