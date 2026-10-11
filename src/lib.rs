pub mod block_builder;
pub mod block_db;
pub mod board;
pub mod builder;
pub mod columnar_builder;
pub mod columnar_db;
pub mod db;
pub mod downloader;
pub mod exporter;
pub mod fen;
pub mod game_db;
pub mod game_downloader;
pub mod id;
pub mod moves;
pub mod record;
pub mod san;
pub mod theme;

pub use block_builder::{BlockBuilderStats, BlockDbBuilder};
pub use block_db::{
    BlockCompressedDb, BlockEntry, BlockPuzzleRecord, DbHeaderV3, DEFAULT_BLOCK_SIZE,
    LPDB_VERSION_V3,
};
pub use board::CompactBoard;
pub use builder::{BuilderStats, DbBuilder};
pub use columnar_builder::{ColumnarBuilderOptions, ColumnarBuilderStats, ColumnarDbBuilder};
pub use columnar_db::{
    BlockEntryV4, ColumnarDb, DbHeaderV4, DEFAULT_V4_BLOCK_SIZE, LPDB_VERSION_V4,
};
pub use db::{DbHeader, Puzzle, PuzzleDatabase, QueryCriteria};
pub use downloader::{download_lichess_puzzle_db, LICHESS_PUZZLE_DB_URL};
pub use exporter::Exporter;
pub use fen::{FenError, PackedFen};
pub use game_db::{
    GameIndexEntry, GameRecord, GameResult, Lgdb, LgdbBuilder, LgdbHeader, LgdbStats, LGDB_MAGIC,
    LGDB_VERSION,
};
pub use game_downloader::{
    download_and_build_lgdb, extract_clean_game_id, find_lichess_token, parse_id_list,
    DownloadProgressStats, GameDownloadOptions,
};
pub use id::{decode_base62_id, encode_base62_id};
pub use moves::{decode_moves, encode_moves, PackedMove};
pub use record::PuzzleRecord;
pub use theme::{ThemeMask, THEME_NAMES};
