# rs_torrent

A BitTorrent client written in Rust, built from the ground up: a bencode parser, `.torrent` file and magnet-link parsing, HTTP/UDP tracker communication, and an async peer-wire-protocol implementation for downloading pieces from peers.

## Features

- **Bencode parser** — zero-copy (`Value`) and owned (`ValueOwned`) parsers built with [`nom`](https://github.com/rust-bakery/nom), including infohash computation for dictionaries.
- **`.torrent` file parsing** — extracts announce URL, piece hashes, piece length, total size and infohash from single- and multi-file torrents.
- **Magnet link parsing** — parses `xt` (infohash), `dn` (display name), `tr` (trackers) and `xl` (size) parameters.
- **Tracker communication** — HTTP(S) and UDP tracker announces (protocol auto-detected from the announce URL), including scrape/completion events.
- **Peer wire protocol** — handshake, keep-alive, and all core messages (`choke`, `unchoke`, `interested`, `not_interested`, `have`, `bitfield`, `request`, `piece`, `cancel`) plus the BEP-10 extension protocol.
- **Metadata exchange (BEP 9)** — fetches the info-dict directly from peers so magnet links can be downloaded without a `.torrent` file.
- **Block-based, pipelined downloading** — pieces are split into 16 KB blocks with multiple in-flight requests per peer for better throughput (see [`docs/BLOCK_BASED_DOWNLOAD.md`](docs/BLOCK_BASED_DOWNLOAD.md) and [`docs/REQUEST_PIPELINING.md`](docs/REQUEST_PIPELINING.md)).
- **SHA-1 piece verification** — every completed piece is checked against its expected hash before being kept (see [`docs/SHA1_VERIFICATION.md`](docs/SHA1_VERIFICATION.md)).
- **Async runtime** — built on [`tokio`](https://tokio.rs), with a peer pool that connects to and manages multiple peers concurrently.

> **Status:** actively developed. The `main` branch already downloads torrents end-to-end from both `.torrent` files and magnet links; some newer areas (e.g. peer-side metadata *upload*, i.e. serving `ut_metadata` to other peers) are stubbed out and still in progress.

## Installation

### Prerequisites

- Rust **1.85+** (this crate uses the 2024 edition — install/update via [rustup](https://rustup.rs)):

  ```bash
  rustup update stable
  ```

### Build from source

```bash
git clone <repository-url>
cd rs_torrent
cargo build --release
```

The CLI binary is produced at `target/release/rs_torrent_cli`.

## Usage

### CLI

The CLI supports two subcommands: downloading from a `.torrent` file, or from a magnet link.

```bash
# Download from a .torrent file
cargo run --release --bin rs_torrent_cli -- file <path/to/file.torrent> -o <output-path>

# Download from a magnet link
cargo run --release --bin rs_torrent_cli -- magnet "<magnet-uri>" -o <output-path>
```

Or, using the built binary directly:

```bash
./target/release/rs_torrent_cli file ubuntu.torrent --output-file ubuntu.iso
./target/release/rs_torrent_cli magnet "magnet:?xt=urn:btih:..." --output-file file.bin
```

Enable logging with `RUST_LOG`:

```bash
RUST_LOG=info ./target/release/rs_torrent_cli file ubuntu.torrent -o ubuntu.iso
```

While downloading, the CLI reports piece/block progress, upload stats, request-pipelining stats, and active peer counts.

### As a library

Add the crate as a path/git dependency, then:

```rust
use rs_torrent::{download_from_torrent_file, download_from_magnet};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // From a .torrent file
    download_from_torrent_file("ubuntu.torrent", "ubuntu.iso").await?;

    // From a magnet link
    download_from_magnet("magnet:?xt=urn:btih:...", "file.bin").await?;

    Ok(())
}
```

#### Parsing a `.torrent` file only

```rust
use rs_torrent::{parse_torrent_file, parse_torrent_bytes};

let torrent = parse_torrent_file("ubuntu.torrent")?;
println!("{} ({} bytes, {} pieces)", torrent.name, torrent.total_size, torrent.pieces.len());

// or from bytes already in memory
let bytes = std::fs::read("ubuntu.torrent")?;
let torrent = parse_torrent_bytes(&bytes)?;
```

#### Parsing a magnet link

```rust
use rs_torrent::torrent::parse_magnet_link;

let magnet = parse_magnet_link("magnet:?xt=urn:btih:...&dn=My+File&tr=udp://tracker.example.com:80")?;
println!("infohash: {:x?}, trackers: {:?}", magnet.infohash, magnet.trackers);
```

#### Parsing raw bencode

```rust
use rs_torrent::{parse, Value};

let parsed = parse(b"d3:cow3:moo4:spam4:eggse")?;
if let Some(Value::Dictionary { entries, .. }) = parsed.first() {
    // entries: FxHashMap<&[u8], Value>
}
```

#### Driving a download manually

For finer control over peer connections and progress polling, use `BitTorrentClient` and `DownloadState` directly — see [`examples/download_client.rs`](examples/download_client.rs) for a full walkthrough (creating a client, connecting to peers, polling progress, and writing the result to disk).

### Examples

Runnable examples live in [`examples/`](examples):

```bash
cargo run --example torrent_parser   # quick-start: parsing .torrent files
cargo run --example torrent_usage    # more thorough parsing examples
cargo run --example download_client  # driving BitTorrentClient manually against test peers
```

## Architecture

```
src/
├── bencode_parser/    # bencode grammar (Value/ValueOwned) + parser combinators (nom)
├── torrent.rs         # .torrent file / info-dict / magnet link parsing (Torrent, Magnet)
├── tracker/
│   ├── http.rs        # HTTP(S) tracker announce/scrape
│   └── udp.rs         # UDP tracker announce (BEP 15)
├── peer/
│   ├── handshake.rs   # BitTorrent handshake
│   ├── message.rs     # wire message encoding/decoding (choke, have, piece, ...)
│   ├── connection.rs  # per-peer TCP connection state
│   ├── metadata.rs    # BEP 9/10 metadata (ut_metadata) exchange for magnet links
│   ├── state.rs       # download state: piece/block tracking + SHA-1 verification
│   ├── pool.rs        # peer pool / concurrency management
│   ├── stats.rs       # pipelining & performance statistics
│   └── client.rs      # BitTorrentClient: ties everything together
└── bin/cli.rs         # `rs_torrent_cli` binary
```

Further design notes:

- [`docs/BLOCK_BASED_DOWNLOAD.md`](docs/BLOCK_BASED_DOWNLOAD.md) — splitting pieces into 16 KB blocks.
- [`docs/REQUEST_PIPELINING.md`](docs/REQUEST_PIPELINING.md) — sending multiple in-flight block requests per peer.
- [`docs/SHA1_VERIFICATION.md`](docs/SHA1_VERIFICATION.md) — verifying completed pieces against the torrent's hashes.

## Testing

```bash
cargo test
```

Unit tests cover the bencode parser, torrent/magnet-link parsing, and download state (block assembly + SHA-1 verification).

## License

This project is licensed under the MIT License.

## Acknowledgments

- Built using the [nom](https://github.com/rust-bakery/nom) parser combinator library
- Inspired by the [nom-bencode](https://github.com/edg-l/nom-bencode) tutorial
- BitTorrent specification: [BEP-0003](http://bittorrent.org/beps/bep_0003.html)
