use rs_torrent::{Torrent, parse_torrent_bytes, parse_torrent_file};
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Torrent Parser Usage Examples\n");

    // Example 1: Parse torrent from file path
    example_1_parse_from_file()?;

    // Example 2: Parse torrent from bytes (useful for network downloads)
    example_2_parse_from_bytes()?;

    // Example 3: Parse multiple torrent files
    example_3_parse_multiple_files()?;

    // Example 4: Extract specific information
    example_4_extract_specific_info()?;

    Ok(())
}

/// Example 1: Parse torrent from file path (most common use case)
fn example_1_parse_from_file() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Example 1: Parse from file path ===");

    let file_path = "test_data/debian-12.11.0-amd64-netinst.iso.torrent";

    if std::path::Path::new(file_path).exists() {
        match parse_torrent_file(file_path) {
            Ok(torrent) => {
                println!("✓ Successfully parsed torrent:");
                print_torrent_info(&torrent);
            }
            Err(e) => {
                println!("✗ Error parsing torrent: {}", e);
            }
        }
    } else {
        println!("Test file not found, skipping example 1");
    }

    println!();
    Ok(())
}

/// Example 2: Parse torrent from bytes (useful when data comes from network)
fn example_2_parse_from_bytes() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Example 2: Parse from bytes ===");

    // Create a simple torrent file as bytes
    let sample_torrent = b"d8:announce19:http://example.com/4:infod4:name9:test.txt12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaa6:lengthi1024eee";

    match parse_torrent_bytes(sample_torrent) {
        Ok(torrent) => {
            println!("✓ Successfully parsed sample torrent:");
            print_torrent_info(&torrent);
        }
        Err(e) => {
            println!("✗ Error parsing sample torrent: {}", e);
        }
    }

    println!();
    Ok(())
}

/// Example 3: Parse multiple torrent files from a directory
fn example_3_parse_multiple_files() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Example 3: Parse multiple files ===");

    let test_dir = "test_data";

    if std::path::Path::new(test_dir).exists() {
        let entries = fs::read_dir(test_dir)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if let Some(extension) = path.extension() {
                if extension == "torrent" {
                    let file_path = path.to_string_lossy();
                    println!("Processing: {}", file_path);

                    match parse_torrent_file(&file_path) {
                        Ok(torrent) => {
                            println!("  ✓ Name: {}", torrent.name);
                            println!("  ✓ Size: {} bytes", torrent.total_size);
                            println!("  ✓ Piece length: {}", torrent.piece_length);
                        }
                        Err(e) => {
                            println!("  ✗ Error: {}", e);
                        }
                    }
                    println!();
                }
            }
        }
    } else {
        println!("Test directory not found, skipping example 3");
    }

    Ok(())
}

/// Example 4: Dynamic parsing with different data sources
fn example_4_extract_specific_info() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Example 4: Extract specific info ===");

    // Simulate reading from different sources
    let sources = vec![
        ("File", "test_data/debian-12.11.0-amd64-netinst.iso.torrent"),
        ("Sample", ""), // We'll use sample data for this
    ];

    for (source_type, path) in sources {
        println!("Source: {}", source_type);

        let result = if source_type == "File" && std::path::Path::new(path).exists() {
            parse_torrent_file(path)
        } else {
            // Use sample data
            let sample = b"d8:announce27:http://tracker.example.com:80804:infod4:name12:example.txt12:piece lengthi32768e6:pieces20:\x12\x34\x56\x78\x9a\xbc\xde\xf0\x12\x34\x56\x78\x9a\xbc\xde\xf0\x12\x34\x56\x786:lengthi2048eee";
            parse_torrent_bytes(sample)
        };

        match result {
            Ok(torrent) => {
                // Extract and display specific information
                println!("  📋 Basic Info:");
                println!("    Name: {}", torrent.name);
                println!(
                    "    Total Size: {} bytes ({:.2} MB)",
                    torrent.total_size,
                    torrent.total_size as f64 / (1024.0 * 1024.0)
                );

                println!("  🔗 Network Info:");
                println!("    Announce URL: {}", torrent.announce);

                println!("  📦 Technical Info:");
                println!(
                    "    Piece Length: {} bytes ({:.2} KB)",
                    torrent.piece_length,
                    torrent.piece_length as f64 / 1024.0
                );

                let num_pieces = (torrent.total_size + torrent.piece_length as u64 - 1)
                    / torrent.piece_length as u64;
                println!("    Estimated Pieces: {}", num_pieces);
            }
            Err(e) => {
                println!("  ✗ Error: {}", e);
            }
        }
        println!();
    }

    Ok(())
}

// Helper function to print torrent information
fn print_torrent_info(torrent: &Torrent) {
    println!("  Name: {}", torrent.name);
    println!("  Announce: {}", torrent.announce);
    println!("  Total Size: {} bytes", torrent.total_size);
    println!("  Piece Length: {} bytes", torrent.piece_length);
}

// Additional utility functions that show advanced usage patterns

/// Parse a torrent and return only the announce URL
pub fn get_announce_url(file_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let torrent = parse_torrent_file(file_path)?;
    Ok(torrent.announce)
}

/// Parse a torrent and return only the file name
pub fn get_torrent_name(file_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let torrent = parse_torrent_file(file_path)?;
    Ok(torrent.name)
}

/// Parse a torrent and return size in different units
pub fn get_torrent_size_info(file_path: &str) -> Result<SizeInfo, Box<dyn std::error::Error>> {
    let torrent = parse_torrent_file(file_path)?;

    Ok(SizeInfo {
        bytes: torrent.total_size,
        kilobytes: torrent.total_size as f64 / 1024.0,
        megabytes: torrent.total_size as f64 / (1024.0 * 1024.0),
        gigabytes: torrent.total_size as f64 / (1024.0 * 1024.0 * 1024.0),
    })
}

#[derive(Debug)]
pub struct SizeInfo {
    pub bytes: u64,
    pub kilobytes: f64,
    pub megabytes: f64,
    pub gigabytes: f64,
}

/// Validate that a file is a valid torrent file
pub fn is_valid_torrent(file_path: &str) -> bool {
    parse_torrent_file(file_path).is_ok()
}

/// Get basic torrent statistics
pub fn get_torrent_stats(file_path: &str) -> Result<TorrentStats, Box<dyn std::error::Error>> {
    let torrent = parse_torrent_file(file_path)?;

    let estimated_pieces =
        (torrent.total_size + torrent.piece_length as u64 - 1) / torrent.piece_length as u64;
    let download_time_estimate = estimate_download_time(torrent.total_size);

    Ok(TorrentStats {
        name: torrent.name,
        size_mb: torrent.total_size as f64 / (1024.0 * 1024.0),
        piece_length_kb: torrent.piece_length as f64 / 1024.0,
        estimated_pieces,
        download_time_estimate,
    })
}

#[derive(Debug)]
pub struct TorrentStats {
    pub name: String,
    pub size_mb: f64,
    pub piece_length_kb: f64,
    pub estimated_pieces: u64,
    pub download_time_estimate: String,
}

fn estimate_download_time(size_bytes: u64) -> String {
    // Rough estimates for different connection speeds
    let size_mb = size_bytes as f64 / (1024.0 * 1024.0);

    // Estimate for different speeds (Mbps)
    let speeds = vec![(1.0, "1 Mbps"), (10.0, "10 Mbps"), (100.0, "100 Mbps")];

    let mut estimates = Vec::new();
    for (speed_mbps, label) in speeds {
        let time_seconds = (size_mb * 8.0) / speed_mbps;
        let time_minutes = time_seconds / 60.0;

        if time_minutes < 60.0 {
            estimates.push(format!("{}: {:.1} min", label, time_minutes));
        } else {
            let time_hours = time_minutes / 60.0;
            estimates.push(format!("{}: {:.1} hr", label, time_hours));
        }
    }

    estimates.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_torrent_parsing() {
        let sample = b"d8:announce19:http://example.com/4:infod4:name9:test.txt12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaa6:lengthi1024eee";

        let result = parse_torrent_bytes(sample);
        assert!(result.is_ok());

        let torrent = result.unwrap();
        assert_eq!(torrent.name, "test.txt");
        assert_eq!(torrent.announce, "http://example.com/");
        assert_eq!(torrent.piece_length, 16384);
        assert_eq!(torrent.total_size, 1024);
    }

    #[test]
    fn test_utility_functions() {
        let sample = b"d8:announce19:http://example.com/4:infod4:name9:test.txt12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaa6:lengthi1024eee";

        // Test with a temporary file
        use std::io::Write;
        let mut temp_file = std::fs::File::create("temp_test.torrent").unwrap();
        temp_file.write_all(sample).unwrap();
        drop(temp_file);

        // Test utility functions
        assert!(is_valid_torrent("temp_test.torrent"));

        let announce = get_announce_url("temp_test.torrent").unwrap();
        assert_eq!(announce, "http://example.com/");

        let name = get_torrent_name("temp_test.torrent").unwrap();
        assert_eq!(name, "test.txt");

        // Clean up
        std::fs::remove_file("temp_test.torrent").unwrap();
    }
}
