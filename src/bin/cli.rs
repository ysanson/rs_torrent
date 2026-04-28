use clap::{Parser, Subcommand};
use rs_torrent::{download_from_magnet, download_from_torrent_file};

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
    #[arg(short, long)]
    output_file: String,
}
#[derive(Subcommand, Debug)]
enum Commands {
    File {
        #[arg(short, long)]
        file: String,
    },
    Magnet {
        #[arg(short, long)]
        magnet: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    env_logger::builder().format_timestamp(None).init();
    if let Commands::File { file } = &args.command {
        download_from_torrent_file(file, &args.output_file).await?;
    } else if let Commands::Magnet { magnet } = &args.command {
        download_from_magnet(magnet, &args.output_file).await?;
    }
    Ok(())
}
