#!/usr/bin/env python3
"""
Interactive PyQt5 Chess Viewer for lipuzzlesdb and LGDB Games.
Calls the compiled Rust 'lpdb' CLI tool via QProcess to query and load
both puzzles and games, with an interactive visual chessboard and move stepper.

Run with:
    python gui_chess_viewer.py
"""

import sys
import os
import json
from pathlib import Path

from PyQt5.QtCore import Qt, QProcess, QTimer, QRectF, pyqtSignal
from PyQt5.QtGui import QPainter, QColor, QFont, QPen, QBrush
from PyQt5.QtWidgets import (
    QApplication, QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
    QGridLayout, QLabel, QPushButton, QSlider, QSpinBox, QLineEdit,
    QComboBox, QTabWidget, QTableWidget, QTableWidgetItem, QHeaderView,
    QSplitter, QFrame, QMessageBox, QFileDialog, QScrollArea, QSizePolicy
)

PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_LPDB = str(PROJECT_ROOT / "data" / "puzzles.lpdb" if (PROJECT_ROOT / "data" / "puzzles.lpdb").exists() else PROJECT_ROOT / "puzzles.lpdb")
DEFAULT_LGDB = str(PROJECT_ROOT / "data" / "games.lgdb" if (PROJECT_ROOT / "data" / "games.lgdb").exists() else PROJECT_ROOT / "games.lgdb")
RELEASE_BIN = str(PROJECT_ROOT / "target" / "release" / "lpdb.exe")
DEBUG_BIN = str(PROJECT_ROOT / "target" / "debug" / "lpdb.exe")


def get_binary_path():
    if os.path.exists(RELEASE_BIN):
        return RELEASE_BIN
    if os.path.exists(DEBUG_BIN):
        return DEBUG_BIN
    return "lpdb"


# ---------------------------------------------------------------------------
# Lightweight Chess Simulator for UCI Moves
# ---------------------------------------------------------------------------

UNICODE_PIECES = {
    'K': '♔', 'Q': '♕', 'R': '♖', 'B': '♗', 'N': '♘', 'P': '♙',
    'k': '♚', 'q': '♛', 'r': '♜', 'b': '♝', 'n': '♞', 'p': '♟',
}

STARTING_FEN = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"


def fen_to_board(fen: str):
    board = [None] * 64
    ranks = fen.split()[0].split('/')
    for r, rank_str in enumerate(ranks):
        f = 0
        for ch in rank_str:
            if ch.isdigit():
                f += int(ch)
            else:
                board[(7 - r) * 8 + f] = ch
                f += 1
    return board


def apply_uci_move(board, move: str):
    if len(move) < 4:
        return list(board)
    new_b = list(board)
    f_file = ord(move[0]) - ord('a')
    f_rank = int(move[1]) - 1
    t_file = ord(move[2]) - ord('a')
    t_rank = int(move[3]) - 1

    from_sq = f_rank * 8 + f_file
    to_sq = t_rank * 8 + t_file
    piece = new_b[from_sq]
    new_b[from_sq] = None

    if not piece:
        return new_b

    # Castling rook moves
    if piece in ('K', 'k') and abs(t_file - f_file) == 2:
        if t_file == 6:  # O-O
            r_from = f_rank * 8 + 7
            r_to = f_rank * 8 + 5
            new_b[r_to] = new_b[r_from]
            new_b[r_from] = None
        elif t_file == 2:  # O-O-O
            r_from = f_rank * 8 + 0
            r_to = f_rank * 8 + 3
            new_b[r_to] = new_b[r_from]
            new_b[r_from] = None

    # En-passant capture
    if piece in ('P', 'p') and f_file != t_file and board[to_sq] is None:
        cap_sq = f_rank * 8 + t_file
        if 0 <= cap_sq < 64:
            new_b[cap_sq] = None

    # Promotion
    if len(move) == 5:
        promo = move[4].upper() if piece.isupper() else move[4].lower()
        new_b[to_sq] = promo
    else:
        new_b[to_sq] = piece

    return new_b


# ---------------------------------------------------------------------------
# Graphical Chessboard Widget
# ---------------------------------------------------------------------------

class ChessBoardWidget(QWidget):
    square_clicked = pyqtSignal(int)

    def __init__(self, parent=None):
        super().__init__(parent)
        self.board = fen_to_board(STARTING_FEN)
        self.last_move = None
        self.flipped = False
        self.setMinimumSize(320, 320)
        self.setSizePolicy(QSizePolicy.Expanding, QSizePolicy.Expanding)

    def set_position(self, board, last_move=None, flipped=False):
        self.board = list(board)
        self.last_move = last_move
        self.flipped = flipped
        self.update()

    def set_fen(self, fen: str, last_move=None, flipped=False):
        self.board = fen_to_board(fen)
        self.last_move = last_move
        self.flipped = flipped
        self.update()

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.Antialiasing)
        painter.setRenderHint(QPainter.TextAntialiasing)

        size = min(self.width(), self.height())
        offset_x = (self.width() - size) // 2
        offset_y = (self.height() - size) // 2
        sq_size = size / 8.0

        light_color = QColor("#f0d9b5")
        dark_color = QColor("#b58863")
        hl_light = QColor("#f7f769")
        hl_dark = QColor("#bacb44")

        # Highlight squares from last_move
        hl_squares = set()
        if self.last_move and len(self.last_move) >= 4:
            f_f = ord(self.last_move[0]) - ord('a')
            f_r = int(self.last_move[1]) - 1
            t_f = ord(self.last_move[2]) - ord('a')
            t_r = int(self.last_move[3]) - 1
            hl_squares.add(f_r * 8 + f_f)
            hl_squares.add(t_r * 8 + t_f)

        font = QFont("Segoe UI Symbol", int(sq_size * 0.65))
        font.setBold(True)
        painter.setFont(font)

        for rank in range(8):
            for file in range(8):
                disp_rank = rank if self.flipped else 7 - rank
                disp_file = 7 - file if self.flipped else file
                sq_idx = disp_rank * 8 + disp_file

                rect = QRectF(offset_x + file * sq_size, offset_y + rank * sq_size, sq_size, sq_size)
                is_light = (file + rank) % 2 == 0

                if sq_idx in hl_squares:
                    c = hl_light if is_light else hl_dark
                else:
                    c = light_color if is_light else dark_color

                painter.fillRect(rect, c)

                # Draw coordinate labels
                coord_font = QFont("Segoe UI", int(sq_size * 0.16), QFont.Bold)
                painter.setFont(coord_font)
                label_color = dark_color if is_light else light_color
                painter.setPen(label_color)

                if file == 0:
                    r_text = str(disp_rank + 1)
                    painter.drawText(rect.adjusted(3, 2, 0, 0), Qt.AlignTop | Qt.AlignLeft, r_text)
                if rank == 7:
                    f_text = chr(ord('a') + disp_file)
                    painter.drawText(rect.adjusted(0, 0, -3, -2), Qt.AlignBottom | Qt.AlignRight, f_text)

                # Draw piece glyph
                painter.setFont(font)
                piece = self.board[sq_idx]
                if piece:
                    glyph = UNICODE_PIECES.get(piece, piece)
                    if piece.isupper():
                        painter.setPen(QColor("#ffffff"))
                        # Subtle drop shadow
                        painter.drawText(rect.adjusted(1, 1, 1, 1), Qt.AlignCenter, glyph)
                        painter.setPen(QColor("#ffffff"))
                    else:
                        painter.setPen(QColor("#18181b"))

                    painter.drawText(rect, Qt.AlignCenter, glyph)

        painter.end()


# ---------------------------------------------------------------------------
# Main Window & Controller
# ---------------------------------------------------------------------------

class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.bin_path = get_binary_path()
        self.current_process = None

        # Data Cache
        self.puzzles = []
        self.games = []

        # Active Replay State
        self.active_board_states = [fen_to_board(STARTING_FEN)]
        self.active_moves = []
        self.active_ply_index = 0
        self.auto_play_timer = QTimer(self)
        self.auto_play_timer.setInterval(600)
        self.auto_play_timer.timeout.connect(self.step_forward)

        self.init_ui()

    def init_ui(self):
        self.setWindowTitle("Chess Database Studio — Puzzles (.lpdb) & Games (.lgdb)")
        self.resize(1180, 780)
        self.setMinimumSize(950, 650)

        self.setStyleSheet("""
            QMainWindow { background-color: #16161c; }
            QWidget { color: #e6edf3; font-family: 'Segoe UI', Arial, sans-serif; font-size: 13px; }
            QTabWidget::pane { border: 1px solid #30363d; background-color: #1e1e24; border-radius: 6px; }
            QTabBar::tab { background: #21262d; color: #8b949e; padding: 9px 18px; border-top-left-radius: 6px; border-top-right-radius: 6px; margin-right: 2px; font-weight: bold; }
            QTabBar::tab:selected { background: #1e1e24; color: #58a6ff; border-bottom: 2px solid #58a6ff; }
            QLineEdit, QSpinBox, QComboBox { background-color: #21262d; border: 1px solid #30363d; border-radius: 5px; padding: 5px 8px; color: #ffffff; }
            QLineEdit:focus, QSpinBox:focus, QComboBox:focus { border: 1px solid #58a6ff; }
            QPushButton { background-color: #238636; color: #ffffff; font-weight: bold; border: none; border-radius: 5px; padding: 7px 15px; }
            QPushButton:hover { background-color: #2ea043; }
            QPushButton:disabled { background-color: #30363d; color: #484f58; }
            QPushButton.secondary { background-color: #30363d; color: #c9d1d9; }
            QPushButton.secondary:hover { background-color: #3c444d; }
            QPushButton.accent { background-color: #1f6feb; }
            QPushButton.accent:hover { background-color: #388bfd; }
            QTableWidget { background-color: #0d1117; border: 1px solid #30363d; border-radius: 6px; gridline-color: #21262d; }
            QHeaderView::section { background-color: #161b22; color: #8b949e; padding: 6px; border: none; font-weight: bold; }
            QFrame.panel { background-color: #1e1e24; border: 1px solid #30363d; border-radius: 8px; }
            QSlider::groove:horizontal { height: 6px; background: #30363d; border-radius: 3px; }
            QSlider::sub-page:horizontal { background: #58a6ff; border-radius: 3px; }
            QSlider::handle:horizontal { background: #ffffff; width: 14px; margin-top: -4px; margin-bottom: -4px; border-radius: 7px; }
        """)

        central = QWidget()
        self.setCentralWidget(central)
        main_layout = QHBoxLayout(central)
        main_layout.setContentsMargins(14, 14, 14, 14)
        main_layout.setSpacing(14)

        splitter = QSplitter(Qt.Horizontal)
        main_layout.addWidget(splitter)

        # Left: Tabs for Puzzles & Games
        tabs = QTabWidget()
        tabs.addTab(self.create_puzzles_tab(), "🧩 Puzzles Database (.lpdb)")
        tabs.addTab(self.create_games_tab(), "♟ Companion Games (.lgdb)")
        splitter.addWidget(tabs)

        # Right: Interactive Chessboard & Replayer
        splitter.addWidget(self.create_chessboard_panel())
        splitter.setStretchFactor(0, 3)
        splitter.setStretchFactor(1, 2)

    # -----------------------------------------------------------------------
    # TAB 1: Puzzles Explorer
    # -----------------------------------------------------------------------
    def create_puzzles_tab(self):
        widget = QWidget()
        layout = QVBoxLayout(widget)
        layout.setContentsMargins(12, 12, 12, 12)
        layout.setSpacing(10)

        # Filter Controls Grid
        filter_box = QFrame()
        filter_box.setProperty("class", "panel")
        g_layout = QGridLayout(filter_box)
        g_layout.setContentsMargins(10, 10, 10, 10)
        g_layout.setSpacing(8)

        # Rating Range
        g_layout.addWidget(QLabel("Min Rating:"), 0, 0)
        self.pz_min_rating = QSpinBox()
        self.pz_min_rating.setRange(300, 3500)
        self.pz_min_rating.setValue(1000)
        g_layout.addWidget(self.pz_min_rating, 0, 1)

        g_layout.addWidget(QLabel("Max Rating:"), 0, 2)
        self.pz_max_rating = QSpinBox()
        self.pz_max_rating.setRange(300, 3500)
        self.pz_max_rating.setValue(1800)
        g_layout.addWidget(self.pz_max_rating, 0, 3)

        # Themes
        g_layout.addWidget(QLabel("Themes:"), 1, 0)
        self.pz_themes_edit = QLineEdit()
        self.pz_themes_edit.setPlaceholderText("e.g. hangingPiece, fork")
        g_layout.addWidget(self.pz_themes_edit, 1, 1, 1, 3)

        # Count
        g_layout.addWidget(QLabel("Limit:"), 2, 0)
        self.pz_limit = QSpinBox()
        self.pz_limit.setRange(1, 500)
        self.pz_limit.setValue(30)
        g_layout.addWidget(self.pz_limit, 2, 1)

        # ID Search
        g_layout.addWidget(QLabel("Find ID:"), 2, 2)
        self.pz_id_edit = QLineEdit()
        self.pz_id_edit.setPlaceholderText("e.g. 00tbH")
        g_layout.addWidget(self.pz_id_edit, 2, 3)

        # Buttons
        btn_layout = QHBoxLayout()
        self.pz_query_btn = QPushButton("🔍 Query Puzzles")
        self.pz_query_btn.clicked.connect(self.query_puzzles)
        btn_layout.addWidget(self.pz_query_btn)

        self.pz_random_btn = QPushButton("🎲 Random")
        self.pz_random_btn.setProperty("class", "accent")
        self.pz_random_btn.clicked.connect(self.query_random_puzzles)
        btn_layout.addWidget(self.pz_random_btn)

        self.pz_find_btn = QPushButton("🔎 Find ID")
        self.pz_find_btn.setProperty("class", "secondary")
        self.pz_find_btn.clicked.connect(self.find_puzzle_by_id)
        btn_layout.addWidget(self.pz_find_btn)

        g_layout.addLayout(btn_layout, 3, 0, 1, 4)
        layout.addWidget(filter_box)

        # Puzzles Table
        self.pz_table = QTableWidget()
        self.pz_table.setColumnCount(5)
        self.pz_table.setHorizontalHeaderLabels(["#", "ID", "Rating", "Themes", "Moves"])
        self.pz_table.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeToContents)
        self.pz_table.horizontalHeader().setSectionResizeMode(1, QHeaderView.ResizeToContents)
        self.pz_table.horizontalHeader().setSectionResizeMode(2, QHeaderView.ResizeToContents)
        self.pz_table.horizontalHeader().setSectionResizeMode(3, QHeaderView.Stretch)
        self.pz_table.horizontalHeader().setSectionResizeMode(4, QHeaderView.ResizeToContents)
        self.pz_table.setSelectionBehavior(QTableWidget.SelectRows)
        self.pz_table.setEditTriggers(QTableWidget.NoEditTriggers)
        self.pz_table.verticalHeader().setVisible(False)
        self.pz_table.itemSelectionChanged.connect(self.on_puzzle_selected)
        layout.addWidget(self.pz_table, stretch=1)

        return widget

    # -----------------------------------------------------------------------
    # TAB 2: Games Viewer
    # -----------------------------------------------------------------------
    def create_games_tab(self):
        widget = QWidget()
        layout = QVBoxLayout(widget)
        layout.setContentsMargins(12, 12, 12, 12)
        layout.setSpacing(10)

        # Filter Box
        filter_box = QFrame()
        filter_box.setProperty("class", "panel")
        g_layout = QGridLayout(filter_box)
        g_layout.setContentsMargins(10, 10, 10, 10)
        g_layout.setSpacing(8)

        g_layout.addWidget(QLabel("Database:"), 0, 0)
        self.gm_db_edit = QLineEdit(DEFAULT_LGDB)
        g_layout.addWidget(self.gm_db_edit, 0, 1, 1, 2)
        browse_btn = QPushButton("Browse...")
        browse_btn.setProperty("class", "secondary")
        browse_btn.clicked.connect(self.browse_games_db)
        g_layout.addWidget(browse_btn, 0, 3)

        g_layout.addWidget(QLabel("Fetch Limit:"), 1, 0)
        self.gm_limit_spin = QSpinBox()
        self.gm_limit_spin.setRange(1, 2000)
        self.gm_limit_spin.setValue(50)
        g_layout.addWidget(self.gm_limit_spin, 1, 1)

        g_layout.addWidget(QLabel("Puzzle / Game ID:"), 1, 2)
        self.gm_search_edit = QLineEdit()
        self.gm_search_edit.setPlaceholderText("e.g. W0kES or m6MZ5eEq")
        g_layout.addWidget(self.gm_search_edit, 1, 3)

        btn_row = QHBoxLayout()
        self.gm_load_btn = QPushButton("📂 Load Games")
        self.gm_load_btn.clicked.connect(self.load_games)
        btn_row.addWidget(self.gm_load_btn)

        self.gm_find_btn = QPushButton("🔎 Search ID")
        self.gm_find_btn.setProperty("class", "secondary")
        self.gm_find_btn.clicked.connect(self.search_game_by_id)
        btn_row.addWidget(self.gm_find_btn)

        g_layout.addLayout(btn_row, 2, 0, 1, 4)
        layout.addWidget(filter_box)

        # Games Table
        self.gm_table = QTableWidget()
        self.gm_table.setColumnCount(6)
        self.gm_table.setHorizontalHeaderLabels(["#", "Game ID", "Puzzle ID", "Date", "Result", "Plies"])
        header = self.gm_table.horizontalHeader()
        header.setSectionResizeMode(0, QHeaderView.ResizeToContents)
        header.setSectionResizeMode(1, QHeaderView.ResizeToContents)
        header.setSectionResizeMode(2, QHeaderView.ResizeToContents)
        header.setSectionResizeMode(3, QHeaderView.ResizeToContents)
        header.setSectionResizeMode(4, QHeaderView.ResizeToContents)
        header.setSectionResizeMode(5, QHeaderView.Stretch)
        self.gm_table.setSelectionBehavior(QTableWidget.SelectRows)
        self.gm_table.setEditTriggers(QTableWidget.NoEditTriggers)
        self.gm_table.verticalHeader().setVisible(False)
        self.gm_table.itemSelectionChanged.connect(self.on_game_selected)
        layout.addWidget(self.gm_table, stretch=1)

        return widget

    # -----------------------------------------------------------------------
    # RIGHT PANE: Chessboard & Move Replayer
    # -----------------------------------------------------------------------
    def create_chessboard_panel(self):
        panel = QFrame()
        panel.setProperty("class", "panel")
        layout = QVBoxLayout(panel)
        layout.setContentsMargins(14, 14, 14, 14)
        layout.setSpacing(10)

        # Header Details Card
        self.meta_card = QFrame()
        self.meta_card.setStyleSheet("background-color: #21262d; border-radius: 6px; padding: 6px;")
        meta_layout = QVBoxLayout(self.meta_card)
        meta_layout.setSpacing(4)
        self.meta_title = QLabel("Select a Puzzle or Game")
        self.meta_title.setStyleSheet("font-size: 15px; font-weight: bold; color: #58a6ff;")
        self.meta_subtitle = QLabel("Positions and moves will display here")
        self.meta_subtitle.setStyleSheet("color: #8b949e; font-size: 12px;")
        meta_layout.addWidget(self.meta_title)
        meta_layout.addWidget(self.meta_subtitle)
        layout.addWidget(self.meta_card)

        # Chessboard
        self.board_widget = ChessBoardWidget()
        layout.addWidget(self.board_widget, stretch=1)

        # Navigation Controls
        nav_box = QVBoxLayout()
        nav_box.setSpacing(6)

        # Move Info & Progress
        info_row = QHBoxLayout()
        self.move_indicator = QLabel("Move 0 / 0")
        self.move_indicator.setStyleSheet("font-weight: bold; color: #e6edf3;")
        self.cur_move_san = QLabel("")
        self.cur_move_san.setStyleSheet("color: #3fb950; font-weight: bold; font-size: 14px;")
        info_row.addWidget(self.move_indicator)
        info_row.addStretch()
        info_row.addWidget(self.cur_move_san)
        nav_box.addLayout(info_row)

        # Slider
        self.ply_slider = QSlider(Qt.Horizontal)
        self.ply_slider.setRange(0, 0)
        self.ply_slider.valueChanged.connect(self.seek_ply)
        nav_box.addWidget(self.ply_slider)

        # Stepper Buttons
        btn_row = QHBoxLayout()
        btn_row.setSpacing(6)

        self.first_btn = QPushButton("⏮ Start")
        self.first_btn.setProperty("class", "secondary")
        self.first_btn.clicked.connect(self.step_first)
        btn_row.addWidget(self.first_btn)

        self.prev_btn = QPushButton("◀ Prev")
        self.prev_btn.setProperty("class", "secondary")
        self.prev_btn.clicked.connect(self.step_back)
        btn_row.addWidget(self.prev_btn)

        self.play_btn = QPushButton("▶ Auto")
        self.play_btn.setProperty("class", "accent")
        self.play_btn.clicked.connect(self.toggle_autoplay)
        btn_row.addWidget(self.play_btn)

        self.next_btn = QPushButton("Next ▶")
        self.next_btn.setProperty("class", "secondary")
        self.next_btn.clicked.connect(self.step_forward)
        btn_row.addWidget(self.next_btn)

        self.last_btn = QPushButton("End ⏭")
        self.last_btn.setProperty("class", "secondary")
        self.last_btn.clicked.connect(self.step_last)
        btn_row.addWidget(self.last_btn)

        self.flip_btn = QPushButton("🔄 Flip")
        self.flip_btn.setProperty("class", "secondary")
        self.flip_btn.clicked.connect(self.toggle_flip)
        btn_row.addWidget(self.flip_btn)

        nav_box.addLayout(btn_row)
        layout.addLayout(nav_box)

        # Companion Game Jump Button
        self.companion_btn = QPushButton("🎬 View Full Game for this Puzzle")
        self.companion_btn.setStyleSheet("background-color: #8957e5; color: #ffffff;")
        self.companion_btn.setVisible(False)
        self.companion_btn.clicked.connect(self.jump_to_companion_game)
        layout.addWidget(self.companion_btn)

        return panel

    # -----------------------------------------------------------------------
    # QProcess Runner & CLI Calls
    # -----------------------------------------------------------------------
    def run_lpdb_cmd(self, args, on_success):
        if self.current_process:
            self.current_process.kill()

        self.current_process = QProcess(self)
        stdout_buf = []

        def handle_stdout():
            data = self.current_process.readAllStandardOutput().data().decode("utf-8", errors="replace")
            stdout_buf.append(data)

        def handle_finished(exit_code, exit_status):
            full_out = "".join(stdout_buf).strip()
            self.current_process = None
            if exit_code == 0:
                on_success(full_out)
            else:
                QMessageBox.warning(self, "Command Error", f"lpdb failed (exit code {exit_code}):\n{full_out}")

        self.current_process.readyReadStandardOutput.connect(handle_stdout)
        self.current_process.finished.connect(handle_finished)
        self.current_process.start(self.bin_path, args)

    # -----------------------------------------------------------------------
    # Puzzle Actions
    # -----------------------------------------------------------------------
    def query_puzzles(self):
        args = [
            "--db", DEFAULT_LPDB,
            "query",
            "--min-rating", str(self.pz_min_rating.value()),
            "--max-rating", str(self.pz_max_rating.value()),
            "--limit", str(self.pz_limit.value()),
            "--json"
        ]
        themes = self.pz_themes_edit.text().strip()
        if themes:
            args.extend(["--themes", themes])

        self.pz_query_btn.setEnabled(False)
        self.pz_query_btn.setText("Querying...")

        def on_done(output):
            self.pz_query_btn.setEnabled(True)
            self.pz_query_btn.setText("🔍 Query Puzzles")
            try:
                self.puzzles = json.loads(output)
                self.populate_puzzles_table(self.puzzles)
            except Exception as e:
                QMessageBox.warning(self, "Parse Error", f"Failed to parse puzzles JSON: {e}")

        self.run_lpdb_cmd(args, on_done)

    def query_random_puzzles(self):
        args = [
            "--db", DEFAULT_LPDB,
            "random",
            "--min-rating", str(self.pz_min_rating.value()),
            "--max-rating", str(self.pz_max_rating.value()),
            "--count", str(self.pz_limit.value()),
            "--json"
        ]
        themes = self.pz_themes_edit.text().strip()
        if themes:
            args.extend(["--themes", themes])

        def on_done(output):
            try:
                self.puzzles = json.loads(output)
                self.populate_puzzles_table(self.puzzles)
            except Exception as e:
                QMessageBox.warning(self, "Parse Error", f"Failed to parse random puzzles JSON: {e}")

        self.run_lpdb_cmd(args, on_done)

    def find_puzzle_by_id(self):
        pid = self.pz_id_edit.text().strip()
        if not pid:
            return
        args = ["--db", DEFAULT_LPDB, "get", "--id", pid, "--json"]

        def on_done(output):
            try:
                p = json.loads(output)
                if p:
                    self.puzzles = [p]
                    self.populate_puzzles_table(self.puzzles)
                else:
                    QMessageBox.information(self, "Not Found", f"Puzzle '{pid}' not found.")
            except Exception as e:
                QMessageBox.warning(self, "Parse Error", f"Failed to parse puzzle: {e}")

        self.run_lpdb_cmd(args, on_done)

    def populate_puzzles_table(self, puzzles):
        self.pz_table.setRowCount(0)
        for i, p in enumerate(puzzles):
            r = self.pz_table.rowCount()
            self.pz_table.insertRow(r)
            self.pz_table.setItem(r, 0, QTableWidgetItem(str(i + 1)))
            self.pz_table.setItem(r, 1, QTableWidgetItem(p.get("id", "")))
            self.pz_table.setItem(r, 2, QTableWidgetItem(str(p.get("rating", ""))))
            self.pz_table.setItem(r, 3, QTableWidgetItem(", ".join(p.get("themes", []))))
            self.pz_table.setItem(r, 4, QTableWidgetItem(str(len(p.get("moves", [])))))

        if puzzles:
            self.pz_table.selectRow(0)

    def on_puzzle_selected(self):
        selected = self.pz_table.selectedItems()
        if not selected:
            return
        row = self.pz_table.currentRow()
        if row < 0 or row >= len(self.puzzles):
            return

        pz = self.puzzles[row]
        fen = pz.get("fen", STARTING_FEN)
        moves = pz.get("moves", [])
        pid = pz.get("id", "")
        rating = pz.get("rating", 0)
        themes = ", ".join(pz.get("themes", []))

        # Check turn in FEN
        parts = fen.split()
        turn_white = parts[1] == 'w' if len(parts) > 1 else True

        self.meta_title.setText(f"Puzzle #{pid} — Elo {rating}")
        self.meta_subtitle.setText(f"Themes: {themes}")
        self.companion_btn.setVisible(True)
        self.companion_btn.setText(f"🎬 View Companion Game for Puzzle #{pid}")

        # Build board states for puzzle solution
        initial_board = fen_to_board(fen)
        states = [initial_board]
        cur = initial_board
        for m in moves:
            cur = apply_uci_move(cur, m)
            states.append(cur)

        self.active_board_states = states
        self.active_moves = moves
        self.active_ply_index = 0
        self.ply_slider.setRange(0, len(moves))
        self.ply_slider.setValue(0)
        self.board_widget.set_position(initial_board, last_move=None, flipped=not turn_white)
        self.update_move_labels()

    # -----------------------------------------------------------------------
    # Games Actions
    # -----------------------------------------------------------------------
    def browse_games_db(self):
        f, _ = QFileDialog.getOpenFileName(self, "Open Game Database", "", "LGDB Files (*.lgdb);;All Files (*)")
        if f:
            self.gm_db_edit.setText(f)

    def load_games(self):
        db_path = self.gm_db_edit.text().strip()
        if not os.path.exists(db_path):
            QMessageBox.warning(self, "File Not Found", f"Database '{db_path}' not found.")
            return

        args = [
            "inspect-games",
            "--games", db_path,
            "--limit", str(self.gm_limit_spin.value()),
            "--json"
        ]

        self.gm_load_btn.setEnabled(False)
        self.gm_load_btn.setText("Loading...")

        def on_done(output):
            self.gm_load_btn.setEnabled(True)
            self.gm_load_btn.setText("📂 Load Games")
            try:
                self.games = json.loads(output)
                self.populate_games_table(self.games)
            except Exception as e:
                QMessageBox.warning(self, "Parse Error", f"Failed to parse games JSON: {e}")

        self.run_lpdb_cmd(args, on_done)

    def search_game_by_id(self):
        target = self.gm_search_edit.text().strip()
        if not target:
            return
        db_path = self.gm_db_edit.text().strip()
        args = ["inspect-games", "--games", db_path, "--puzzle-id", target, "--json"]

        def on_done_puzzle(output):
            try:
                g = json.loads(output)
                if g:
                    self.games = [g]
                    self.populate_games_table(self.games)
                    return
            except Exception:
                pass
            # Try game ID lookup
            args2 = ["inspect-games", "--games", db_path, "--game-id", target, "--json"]
            self.run_lpdb_cmd(args2, on_done_game)

        def on_done_game(output2):
            try:
                g = json.loads(output2)
                if g:
                    self.games = [g]
                    self.populate_games_table(self.games)
                else:
                    QMessageBox.information(self, "Not Found", f"No game found matching '{target}'.")
            except Exception as e:
                QMessageBox.warning(self, "Error", str(e))

        self.run_lpdb_cmd(args, on_done_puzzle)

    def populate_games_table(self, games):
        self.gm_table.setRowCount(0)
        for i, g in enumerate(games):
            r = self.gm_table.rowCount()
            self.gm_table.insertRow(r)
            self.gm_table.setItem(r, 0, QTableWidgetItem(str(i + 1)))
            self.gm_table.setItem(r, 1, QTableWidgetItem(g.get("game_id", "")))
            self.gm_table.setItem(r, 2, QTableWidgetItem(g.get("puzzle_id", "")))
            self.gm_table.setItem(r, 3, QTableWidgetItem(g.get("date", "")))
            self.gm_table.setItem(r, 4, QTableWidgetItem(g.get("result", "")))
            self.gm_table.setItem(r, 5, QTableWidgetItem(str(g.get("plies", 0))))

        if games:
            self.gm_table.selectRow(0)

    def on_game_selected(self):
        row = self.gm_table.currentRow()
        if row < 0 or row >= len(self.games):
            return

        g = self.games[row]
        gid = g.get("game_id", "")
        pid = g.get("puzzle_id", "")
        date = g.get("date", "????.??.??")
        result = g.get("result", "*")
        moves = g.get("moves", [])

        self.meta_title.setText(f"Game {gid} — Result: {result}")
        subtitle = f"Date: {date} | Total Plies: {len(moves)}"
        if pid:
            subtitle += f" | Associated Puzzle: #{pid}"
        self.meta_subtitle.setText(subtitle)
        self.companion_btn.setVisible(False)

        # Build full game board states from standard startpos
        states = [fen_to_board(STARTING_FEN)]
        cur = states[0]
        for m in moves:
            cur = apply_uci_move(cur, m)
            states.append(cur)

        self.active_board_states = states
        self.active_moves = moves
        self.active_ply_index = 0
        self.ply_slider.setRange(0, len(moves))
        self.ply_slider.setValue(0)
        self.board_widget.set_position(states[0], last_move=None, flipped=False)
        self.update_move_labels()

    def jump_to_companion_game(self):
        row = self.pz_table.currentRow()
        if row < 0 or row >= len(self.puzzles):
            return
        pid = self.puzzles[row].get("id", "")
        if not pid:
            return

        # Switch to games tab and trigger search
        tab_widget = self.centralWidget().findChild(QTabWidget)
        if tab_widget:
            tab_widget.setCurrentIndex(1)
        self.gm_search_edit.setText(pid)
        self.search_game_by_id()

    # -----------------------------------------------------------------------
    # Move Navigation & Replay
    # -----------------------------------------------------------------------
    def seek_ply(self, ply):
        if 0 <= ply < len(self.active_board_states):
            self.active_ply_index = ply
            last_m = self.active_moves[ply - 1] if ply > 0 else None
            self.board_widget.set_position(self.active_board_states[ply], last_move=last_m, flipped=self.board_widget.flipped)
            self.update_move_labels()

    def step_first(self):
        self.ply_slider.setValue(0)

    def step_last(self):
        self.ply_slider.setValue(len(self.active_moves))

    def step_forward(self):
        if self.active_ply_index < len(self.active_moves):
            self.ply_slider.setValue(self.active_ply_index + 1)
        else:
            self.auto_play_timer.stop()
            self.play_btn.setText("▶ Auto")

    def step_back(self):
        if self.active_ply_index > 0:
            self.ply_slider.setValue(self.active_ply_index - 1)

    def toggle_autoplay(self):
        if self.auto_play_timer.isActive():
            self.auto_play_timer.stop()
            self.play_btn.setText("▶ Auto")
        else:
            if self.active_ply_index >= len(self.active_moves):
                self.ply_slider.setValue(0)
            self.auto_play_timer.start()
            self.play_btn.setText("⏸ Pause")

    def toggle_flip(self):
        self.board_widget.flipped = not self.board_widget.flipped
        last_m = self.active_moves[self.active_ply_index - 1] if self.active_ply_index > 0 else None
        self.board_widget.set_position(
            self.active_board_states[self.active_ply_index],
            last_move=last_m,
            flipped=self.board_widget.flipped
        )

    def update_move_labels(self):
        total = len(self.active_moves)
        cur = self.active_ply_index
        self.move_indicator.setText(f"Ply {cur} / {total}")
        if cur > 0 and cur <= total:
            fullmove = (cur + 1) // 2
            turn = "..." if cur % 2 == 0 else "."
            m_uci = self.active_moves[cur - 1]
            self.cur_move_san.setText(f"{fullmove}{turn} {m_uci}")
        else:
            self.cur_move_san.setText("Start Position")


def main():
    app = QApplication(sys.argv)
    win = MainWindow()
    win.show()
    sys.exit(app.exec_())


if __name__ == "__main__":
    main()
