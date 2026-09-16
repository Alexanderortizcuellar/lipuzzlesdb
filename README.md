# lipuzzlesdb (LPDB V4)

[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

An ultra-compact, high-performance columnar seekable chess puzzles database engine and streaming ingestion tool written in Rust.

Designed for **instant random access**, **sub-microsecond cached queries**, and a **minimal storage footprint** for mobile apps (iOS / Android / Flutter / React Native), embedded systems, and desktop applications.

---

## 🚀 Performance & Compression Benchmarks

Compressing the **entire 6.01+ Million Lichess Puzzle Database** down to an ultra-compact binary format with **zero loss of chess data**:

| Database Version / Filter | Puzzle Count | Disk Size | Size Reduction vs SQLite | Cold Seek Latency | Cached Query Latency |
|---|---|---|---|---|---|
| **Original SQLite** | 6,014,381 | **818.26 MB** | Baseline (0%) | ~5–50 ms | ~1 ms |
| **LPDB V1 (Fixed 64-Byte Record)** | 6,014,381 | **420.23 MB** | -48.6% | ~0.2 µs | ~0.2 µs |
| **LPDB V2 (Fixed 40-Byte Record)** | 6,014,381 | **283.63 MB** | -65.3% | ~0.2 µs | ~0.2 µs |
| **LPDB V3 (Row-Block Zstd)** | 6,014,381 | **209.26 MB** | -74.4% | ~0.08 ms | ~0.3 µs |
| **LPDB V4 Full Database (Columnar Zstd-19)** | **6,014,381** | **168.05 MB** | **-79.5%** | **~0.6 ms** | **< 0.5 µs** |
| **LPDB V4 (1000–2400 Elo)** | **4,336,863** | **123.39 MB** | **-84.9%** | **~0.6 ms** | **< 0.5 µs** |
| **LPDB V4 (1300–2500 Elo)** | **3,227,532** | **94.07 MB** | **-88.5%** | **~0.6 ms** | **< 0.5 µs** |
| **LPDB V4 (1200–2000 Elo)** | **2,657,383** | **75.85 MB** | **-90.7%** | **~0.6 ms** | **< 0.5 µs** |

> ⚡ **Speed Summary**: Cold block decompressions take **~0.5 ms** (reading 2,048 puzzles at once). Once cached in the built-in 16-block LRU cache, queries execute in **< 0.5 microseconds** (over 2 million puzzles / sec).

---

## 🎯 Key Features

1. **One-Command Stream Ingestion**:
   Stream, parse, filter, and pack directly from `database.lichess.org/lichess_db_puzzle.csv.zst` without uncompressing 1.8 GB CSV files to your disk.
2. **Columnar Block Storage**:
   Data within each 2,048-puzzle block is partitioned into contiguous columns (Base62 IDs, delta-encoded Elo ratings, theme dictionary indices, compact occupancy bitboards, piece nibbles, and move streams) for maximum Zstandard entropy reduction.
3. **100% Data Fidelity & Lossless Fallbacks**:
   Compact 24-byte bitboards for standard chess boards, with an adaptive block overflow pool for unusual pawn counts or non-standard IDs.
4. **Instant Zero-Copy MMap**:
   Zero startup overhead — opening a 6M puzzle database takes **~0.06 ms**.
5. **Rich Queries & Random Sampling**:
   Filter puzzles by rating range, theme bitmasks (supports all 73 Lichess themes), popularity, and minimum plays.
6. **Built-in Sample Database**:
   A lightweight `sample_puzzles.lpdb` (**150,000 puzzles, ~4.26 MB**) is included directly in the repository for immediate testing, benchmarking, and development.

---

## 📦 Installation & Quick Start

### Build from Source

Ensure you have Rust and Cargo installed:

```bash
git clone https://github.com/Alexanderortizcuellar/lipuzzlesdb.git
cd lipuzzlesdb
cargo build --release
```

The executable will be located at `./target/release/lpdb` (or `lpdb.exe` on Windows).

---

## 🛠️ CLI Usage Examples

### 1. Test with the Included Sample Database

```bash
# View database statistics and metadata
./target/release/lpdb --db sample_puzzles.lpdb info

# Fetch a puzzle by ID (with ASCII board rendering)
./target/release/lpdb --db sample_puzzles.lpdb get --id 00008 --render

# Pick a random puzzle in the 1500-1600 Elo range tagged with 'fork'
./target/release/lpdb --db sample_puzzles.lpdb random --min-rating 1500 --max-rating 1600 --themes "fork" --render

# Run performance benchmarks
./target/release/lpdb --db sample_puzzles.lpdb bench --iterations 10000
```

---

### 2. Build Your Own Database

#### Option A: One-Command Download & Build from Lichess (Zero Temp Files)
```bash
lpdb build --download --output puzzles.lpdb --compression-level 19
```

#### Option B: Build Filtered Mobile Dataset (e.g. 1300–2500 Elo under 95 MB)
```bash
lpdb build --download --output alex_puzzles.lpdb --min-rating 1300 --max-rating 2500
```

#### Option C: Build from Local SQLite File
```bash
lpdb build --sqlite lichess_puzzles.db --output puzzles.lpdb
```

#### Option D: Build from Local `.csv.zst`
```bash
lpdb build --csv-zst lichess_db_puzzle.csv.zst --output puzzles.lpdb
```

---

### 3. Querying & Exporting via CLI

```bash
# Query puzzles matching theme and rating
lpdb --db puzzles.lpdb query --min-rating 1500 --max-rating 1600 --themes "fork,endgame" --limit 10

# Export filtered puzzles directly to PGN (e.g. 600 hangingPiece puzzles for practice)
lpdb --db puzzles.lpdb export --themes hangingPiece --limit 600 --output hanging_pieces.pgn

# Export filtered puzzles to CSV
lpdb --db puzzles.lpdb export --min-rating 1400 --max-rating 1800 --themes "fork" --limit 500 --output forks.csv

# Output random puzzle in PGN format
lpdb --db puzzles.lpdb random --min-rating 1800 --max-rating 2000 --themes "endgame" --pgn

# Lookup puzzle by ID
lpdb --db puzzles.lpdb get --id 00008 --board
```

---


## 🧩 Rust API Example

Add `lipuzzlesdb` to your `Cargo.toml`:

```rust
use lipuzzles::{ColumnarDb, QueryCriteria, ThemeMask};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open memory-mapped database with auto-cached decompression
    let db = ColumnarDb::open("alex_puzzles.lpdb")?;
    println!("Total puzzles loaded: {}", db.len());

    // 1. O(1) direct ID lookup
    if let Some(puzzle) = db.get_by_id("00008")? {
        println!("ID: {}", puzzle.id);
        println!("FEN: {}", puzzle.fen);
        println!("Moves: {:?}", puzzle.moves);
        println!("Rating: {}", puzzle.rating);
    }

    // 2. Filtered random puzzle
    let mut criteria = QueryCriteria::default();
    criteria.min_rating = Some(1500);
    criteria.max_rating = Some(1700);
    criteria.themes = Some(ThemeMask::from_csv("fork,short"));

    if let Some(random_puzzle) = db.random_puzzle(&criteria)? {
        println!("Found random puzzle: {}", random_puzzle.id);
    }

    Ok(())
}
```

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).
