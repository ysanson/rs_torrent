use clap::{Parser, Subcommand};
use rs_torrent::{download_from_magnet, download_from_torrent_file};

/// Torrent client downloader CLI. Supports downloading from torrent files and magnet URIs.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand, Debug)]
enum Commands {
    File {
        /// The torrent file to download
        file: String,
        #[arg(short, long)]
        output_file: String,
    },
    Magnet {
        /// The magnet URI to download
        magnet: String,
        #[arg(short, long)]
        output_file: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    env_logger::builder().format_timestamp(None).init();
    if let Commands::File { file, output_file } = &args.command {
        download_from_torrent_file(file, output_file).await?;
    } else if let Commands::Magnet {
        magnet,
        output_file,
    } = &args.command
    {
        download_from_magnet(magnet, output_file).await?;
    }
    Ok(())
}
