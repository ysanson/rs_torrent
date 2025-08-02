use clap::Parser;
use rs_torrent::download_from_torrent_file;

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The file to download the torrent.
    #[arg(short, long)]
    file: String,
    #[arg(short, long)]
    output_file: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    // println!("Downloading torrent from {}", args.file);
    download_from_torrent_file(&args.file, &args.output_file).await?;
    Ok(())
}
