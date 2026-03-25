use rs_torrent::{parse_torrent_bytes, parse_torrent_file};
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Quick Start Guide: Using the Torrent Parser\n");

    // 1. Parse from a file path (most common use case)
    println!("📁 Method 1: Parse from file");
    let file_path = "test_data/debian-12.11.0-amd64-netinst.iso.torrent";

    if std::path::Path::new(file_path).exists() {
        match parse_torrent_file(file_path) {
            Ok(torrent) => {
                println!("  ✅ Success!");
                println!("  📄 Name: {}", torrent.name);
                println!("  🔗 Tracker: {}", torrent.announce);
                println!(
                    "  📏 Size: {:.2} MB",
                    torrent.total_size as f64 / (1024.0 * 1024.0)
                );
            }
            Err(e) => println!("  ❌ Error: {e}"),
        }
    } else {
        println!("  ⚠️  Test file not found");
    }

    println!();

    // 2. Parse from bytes (useful for network downloads or embedded data)
    println!("💾 Method 2: Parse from bytes");

    // Read a file into bytes first (simulating network download)
    if std::path::Path::new(file_path).exists() {
        let file_bytes = fs::read(file_path)?;

        match parse_torrent_bytes(&file_bytes) {
            Ok(torrent) => {
                println!("  ✅ Success!");
                println!("  📄 Name: {}", torrent.name);
                println!("  🧩 Pieces: {} KB each", torrent.piece_length / 1024);
            }
            Err(e) => println!("  ❌ Error: {e}"),
        }
    }

    println!();

    // 3. Quick utility functions
    println!("🛠️  Method 3: Quick utilities");

    if std::path::Path::new(file_path).exists() {
        // Just get the name
        if let Ok(name) = get_torrent_name(file_path) {
            println!("  📄 Quick name lookup: {name}");
        }

        // Just get the announce URL
        if let Ok(announce) = get_announce_url(file_path) {
            println!("  🔗 Quick tracker lookup: {announce}");
        }

        // Check if file is valid
        if is_valid_torrent(file_path) {
            println!("  ✅ File is a valid torrent");
        }
    }

    println!();

    // 4. Error handling example
    println!("⚠️  Method 4: Error handling");

    match parse_torrent_file("nonexistent.torrent") {
        Ok(torrent) => {
            println!("  This shouldn't happen: {torrent:?}");
        }
        Err(e) => {
            println!("  ✅ Properly caught error: {e}");
            println!("  💡 Always handle errors when parsing dynamic files!");
        }
    }

    println!();
    println!("🎯 Summary:");
    println!("   • Use `parse_torrent_file(path)` for files");
    println!("   • Use `parse_torrent_bytes(&bytes)` for byte data");
    println!("   • Always handle errors with match/Result");
    println!("   • The parser returns TorrentInfo with basic fields");

    Ok(())
}

// Utility functions for quick operations

/// Get just the torrent name
fn get_torrent_name(file_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let torrent = parse_torrent_file(file_path)?;
    Ok(torrent.name)
}

/// Get just the announce URL
fn get_announce_url(file_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let torrent = parse_torrent_file(file_path)?;
    Ok(torrent.announce)
}

/// Check if a file is a valid torrent
fn is_valid_torrent(file_path: &str) -> bool {
    parse_torrent_file(file_path).is_ok()
}

/// Example of parsing and extracting all useful info at once
#[allow(dead_code)]
fn extract_all_info(file_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let torrent = parse_torrent_file(file_path)?;

    println!("📋 Complete Torrent Information:");
    println!("  Name: {}", torrent.name);
    println!("  Announce: {}", torrent.announce);
    println!("  Total Size: {} bytes", torrent.total_size);
    println!("  Piece Length: {} bytes", torrent.piece_length);

    // Calculate some derived info
    let size_mb = torrent.total_size as f64 / (1024.0 * 1024.0);
    let piece_size_kb = torrent.piece_length as f64 / 1024.0;
    let estimated_pieces =
        torrent.total_size.div_ceil(torrent.piece_length as u64);

    println!("  Size (MB): {size_mb:.2}");
    println!("  Piece Size (KB): {piece_size_kb:.1}");
    println!("  Estimated Pieces: {estimated_pieces}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_utility_functions() {
        // This test will only pass if the test file exists
        let test_file = "test_data/debian-12.11.0-amd64-netinst.iso.torrent";

        if std::path::Path::new(test_file).exists() {
            assert!(is_valid_torrent(test_file));
            assert!(get_torrent_name(test_file).is_ok());
            assert!(get_announce_url(test_file).is_ok());
        }
    }

    #[test]
    fn test_invalid_file() {
        assert!(!is_valid_torrent("nonexistent.torrent"));
        assert!(get_torrent_name("nonexistent.torrent").is_err());
        assert!(get_announce_url("nonexistent.torrent").is_err());
    }
}
