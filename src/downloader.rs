//! Downloader module for fetching the latest official Lichess puzzle database.

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;
use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};

pub const LICHESS_PUZZLE_DB_URL: &str = "https://database.lichess.org/lichess_db_puzzle.csv.zst";

/// Downloads the latest Lichess puzzle CSV.ZST file with a progress bar.
pub fn download_lichess_puzzle_db(
    dest_path: impl AsRef<Path>,
    custom_url: Option<&str>,
) -> io::Result<u64> {
    let url = custom_url.unwrap_or(LICHESS_PUZZLE_DB_URL);
    println!("Connecting to {}...", url);

    let response = ureq::get(url)
        .call()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("HTTP request failed: {}", e)))?;

    let total_size = response
        .header("Content-Length")
        .and_then(|len| len.parse::<u64>().ok())
        .unwrap_or(0);

    let pb = if total_size > 0 {
        let p = ProgressBar::new(total_size);
        p.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
                .unwrap()
                .progress_chars("#>-"),
        );
        p
    } else {
        let p = ProgressBar::new_spinner();
        p.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} [{elapsed_precise}] {bytes} downloaded ({bytes_per_sec})")
                .unwrap(),
        );
        p
    };

    println!("Downloading to {:?}...", dest_path.as_ref());
    let mut reader = response.into_reader();
    let file = File::create(dest_path.as_ref())?;
    let mut writer = BufWriter::with_capacity(1024 * 1024, file);

    let mut buffer = [0u8; 64 * 1024];
    let mut downloaded = 0u64;
    let t0 = Instant::now();

    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buffer[..n])?;
        downloaded += n as u64;
        pb.set_position(downloaded);
    }

    writer.flush()?;
    pb.finish_with_message("Download complete");
    let dur = t0.elapsed();

    println!(
        "Successfully downloaded {:.2} MB in {:.1}s ({:.2} MB/s)",
        downloaded as f64 / (1024.0 * 1024.0),
        dur.as_secs_f64(),
        (downloaded as f64 / (1024.0 * 1024.0)) / dur.as_secs_f64()
    );

    Ok(downloaded)
}
