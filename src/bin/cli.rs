use clap::Parser;

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The file to download the torrent.
    #[arg(short, long)]
    file: String,
}

fn main() {
    let args = Args::parse();
    println!("Downloading torrent from {}", args.file);
}
