#!/usr/bin/env python3
"""
PyQt5 GUI for downloading real games from Lichess based on puzzles,
packing them into the ultra-compressed .lgdb (Lichess Game Database) format.

Features:
- Live progress bar, ETA, speed, and real-time metrics
- User Pause / Resume controls
- Configurable automatic cooldown pause (e.g. pause 3s every 50 games)
- Rating range filtering (e.g. 1000 to 1800)
- Target games or target database size in MB
- Live activity feed showing downloaded games
"""

import sys
import os
import json
import time
import subprocess
from pathlib import Path

from PyQt5.QtCore import Qt, QThread, pyqtSignal, QTimer
from PyQt5.QtWidgets import (
    QApplication, QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
    QGridLayout, QLabel, QPushButton, QProgressBar, QSpinBox,
    QDoubleSpinBox, QLineEdit, QFileDialog, QGroupBox, QTableWidget,
    QTableWidgetItem, QHeaderView, QFrame
)

try:
    import psutil
except ImportError:
    psutil = None

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


class DownloadWorker(QThread):
    progress_signal = pyqtSignal(dict)
    cooldown_signal = pyqtSignal(dict)
    log_signal = pyqtSignal(str)
    complete_signal = pyqtSignal(dict)
    error_signal = pyqtSignal(str)

    def __init__(self, cmd_args):
        super().__init__()
        self.cmd_args = cmd_args
        self.process = None
        self.is_paused = False
        self.is_stopped = False

    def run(self):
        try:
            self.log_signal.emit(f"Launching downloader process: {' '.join(self.cmd_args)}")
            self.process = subprocess.Popen(
                self.cmd_args,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
                creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0
            )

            for line in iter(self.process.stdout.readline, ''):
                if self.is_stopped:
                    break
                line_str = line.strip()
                if not line_str:
                    continue

                if line_str.startswith('{') and line_str.endswith('}'):
                    try:
                        data = json.loads(line_str)
                        msg_type = data.get("type")
                        if msg_type == "progress":
                            self.progress_signal.emit(data)
                        elif msg_type == "cooldown":
                            self.cooldown_signal.emit(data)
                        elif msg_type == "complete":
                            self.complete_signal.emit(data)
                    except json.JSONDecodeError:
                        self.log_signal.emit(line_str)
                else:
                    self.log_signal.emit(line_str)

            self.process.stdout.close()
            self.process.wait()

        except Exception as e:
            self.error_signal.emit(str(e))

    def toggle_pause(self):
        if not self.process:
            return False
        pid = self.process.pid
        if psutil:
            p = psutil.Process(pid)
            if self.is_paused:
                p.resume()
                self.is_paused = False
            else:
                p.suspend()
                self.is_paused = True
            return self.is_paused
        return False

    def stop(self):
        self.is_stopped = True
        if self.process:
            if self.is_paused and psutil:
                try:
                    psutil.Process(self.process.pid).resume()
                except Exception:
                    pass
            self.process.terminate()
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.kill()


class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.worker = None
        self.start_time = None
        self.last_count = 0
        self.last_time = 0

        self.init_ui()

    def init_ui(self):
        self.setWindowTitle("Lichess Game Database Downloader (LGDB)")
        self.resize(920, 680)
        self.setMinimumSize(800, 600)

        # Dark theme stylesheet
        self.setStyleSheet("""
            QMainWindow {
                background-color: #1e1e24;
            }
            QWidget {
                color: #e0e0e0;
                font-family: 'Segoe UI', Arial, sans-serif;
                font-size: 13px;
            }
            QGroupBox {
                border: 1px solid #3c3c46;
                border-radius: 8px;
                margin-top: 14px;
                padding-top: 16px;
                font-weight: bold;
                color: #58a6ff;
            }
            QGroupBox::title {
                subcontrol-origin: margin;
                left: 12px;
                padding: 0 6px;
            }
            QLineEdit, QSpinBox, QDoubleSpinBox {
                background-color: #2b2b36;
                border: 1px solid #444455;
                border-radius: 5px;
                padding: 5px 8px;
                color: #ffffff;
            }
            QLineEdit:focus, QSpinBox:focus, QDoubleSpinBox:focus {
                border: 1px solid #58a6ff;
            }
            QPushButton {
                background-color: #238636;
                color: #ffffff;
                font-weight: bold;
                border: none;
                border-radius: 6px;
                padding: 9px 18px;
                font-size: 14px;
            }
            QPushButton:hover {
                background-color: #2ea043;
            }
            QPushButton:disabled {
                background-color: #383842;
                color: #6e6e78;
            }
            QPushButton#pauseBtn {
                background-color: #d29922;
            }
            QPushButton#pauseBtn:hover {
                background-color: #e3b341;
            }
            QPushButton#stopBtn {
                background-color: #da3633;
            }
            QPushButton#stopBtn:hover {
                background-color: #f85149;
            }
            QProgressBar {
                border: 1px solid #3c3c46;
                border-radius: 6px;
                background-color: #2b2b36;
                text-align: center;
                color: #ffffff;
                font-weight: bold;
                height: 24px;
            }
            QProgressBar::chunk {
                background: qlineargradient(x1:0, y1:0, x2:1, y2:0, stop:0 #1f6feb, stop:1 #58a6ff);
                border-radius: 5px;
            }
            QTableWidget {
                background-color: #16161c;
                border: 1px solid #3c3c46;
                border-radius: 6px;
                gridline-color: #2b2b36;
            }
            QHeaderView::section {
                background-color: #252530;
                color: #8b949e;
                padding: 6px;
                border: none;
                font-weight: bold;
            }
            QFrame.card {
                background-color: #252530;
                border: 1px solid #3c3c46;
                border-radius: 8px;
                padding: 10px;
            }
        """)

        central_widget = QWidget()
        self.setCentralWidget(central_widget)
        main_layout = QVBoxLayout(central_widget)
        main_layout.setContentsMargins(18, 16, 18, 16)
        main_layout.setSpacing(14)

        # Title / Banner
        banner_layout = QHBoxLayout()
        title_label = QLabel("⚡ LGDB Companion Game Downloader")
        title_label.setStyleSheet("font-size: 20px; font-weight: bold; color: #ffffff;")
        badge_label = QLabel("Ultra-Compact 16-Bit Format")
        badge_label.setStyleSheet("background-color: #21262d; border: 1px solid #30363d; border-radius: 4px; padding: 4px 8px; color: #58a6ff; font-weight: bold;")
        banner_layout.addWidget(title_label)
        banner_layout.addStretch()
        banner_layout.addWidget(badge_label)
        main_layout.addLayout(banner_layout)

        # Configuration Group
        cfg_group = QGroupBox("Download Configuration")
        cfg_layout = QGridLayout(cfg_group)
        cfg_layout.setContentsMargins(14, 14, 14, 14)
        cfg_layout.setHorizontalSpacing(16)
        cfg_layout.setVerticalSpacing(10)

        # Source DB
        cfg_layout.addWidget(QLabel("Source Puzzle DB:"), 0, 0)
        self.source_db_edit = QLineEdit(DEFAULT_LPDB)
        cfg_layout.addWidget(self.source_db_edit, 0, 1, 1, 3)
        self.browse_db_btn = QPushButton("Browse...")
        self.browse_db_btn.setStyleSheet("padding: 5px 12px; font-size: 12px; background-color: #30363d;")
        self.browse_db_btn.clicked.connect(self.browse_source_db)
        cfg_layout.addWidget(self.browse_db_btn, 0, 4)

        # Rating Range
        cfg_layout.addWidget(QLabel("Min Rating:"), 1, 0)
        self.min_rating_spin = QSpinBox()
        self.min_rating_spin.setRange(300, 3500)
        self.min_rating_spin.setValue(1000)
        cfg_layout.addWidget(self.min_rating_spin, 1, 1)

        cfg_layout.addWidget(QLabel("Max Rating:"), 1, 2)
        self.max_rating_spin = QSpinBox()
        self.max_rating_spin.setRange(300, 3500)
        self.max_rating_spin.setValue(1800)
        cfg_layout.addWidget(self.max_rating_spin, 1, 3)

        # Target Games / Size
        cfg_layout.addWidget(QLabel("Target Games:"), 2, 0)
        self.target_games_spin = QSpinBox()
        self.target_games_spin.setRange(10, 1_000_000)
        self.target_games_spin.setValue(10_000)
        self.target_games_spin.setSingleStep(500)
        self.target_games_spin.valueChanged.connect(self.update_target_est)
        cfg_layout.addWidget(self.target_games_spin, 2, 1)

        self.est_size_label = QLabel("≈ 2.0 MB (.lgdb)")
        self.est_size_label.setStyleSheet("color: #8b949e; font-style: italic;")
        cfg_layout.addWidget(self.est_size_label, 2, 2)

        # Cooldown Pause
        cfg_layout.addWidget(QLabel("Pause Every:"), 3, 0)
        self.pause_every_spin = QSpinBox()
        self.pause_every_spin.setRange(0, 1000)
        self.pause_every_spin.setValue(50)
        self.pause_every_spin.setSuffix(" games")
        cfg_layout.addWidget(self.pause_every_spin, 3, 1)

        cfg_layout.addWidget(QLabel("Pause For:"), 3, 2)
        self.pause_sec_spin = QDoubleSpinBox()
        self.pause_sec_spin.setRange(0.5, 30.0)
        self.pause_sec_spin.setValue(3.0)
        self.pause_sec_spin.setSingleStep(0.5)
        self.pause_sec_spin.setSuffix(" sec")
        cfg_layout.addWidget(self.pause_sec_spin, 3, 3)

        # Output .lgdb
        cfg_layout.addWidget(QLabel("Output File:"), 4, 0)
        self.output_edit = QLineEdit(DEFAULT_LGDB)
        cfg_layout.addWidget(self.output_edit, 4, 1, 1, 3)
        self.browse_out_btn = QPushButton("Browse...")
        self.browse_out_btn.setStyleSheet("padding: 5px 12px; font-size: 12px; background-color: #30363d;")
        self.browse_out_btn.clicked.connect(self.browse_output_file)
        cfg_layout.addWidget(self.browse_out_btn, 4, 4)

        main_layout.addWidget(cfg_group)

        # Metrics Dashboard Cards
        metrics_layout = QHBoxLayout()
        metrics_layout.setSpacing(12)

        self.card_downloaded = self.create_metric_card("Downloaded", "0", "#3fb950")
        self.card_plies = self.create_metric_card("Total Plies", "0", "#58a6ff")
        self.card_size = self.create_metric_card("Est. File Size", "0.0 KB", "#d29922")
        self.card_speed = self.create_metric_card("Speed", "0.0 g/s", "#bc8cff")

        metrics_layout.addWidget(self.card_downloaded)
        metrics_layout.addWidget(self.card_plies)
        metrics_layout.addWidget(self.card_size)
        metrics_layout.addWidget(self.card_speed)
        main_layout.addLayout(metrics_layout)

        # Progress Section
        prog_layout = QVBoxLayout()
        prog_layout.setSpacing(6)

        status_row = QHBoxLayout()
        self.status_label = QLabel("Status: Ready to start")
        self.status_label.setStyleSheet("font-weight: bold; color: #8b949e;")
        self.time_label = QLabel("Elapsed: 00:00:00 | ETA: --:--:--")
        self.time_label.setStyleSheet("color: #8b949e;")
        status_row.addWidget(self.status_label)
        status_row.addStretch()
        status_row.addWidget(self.time_label)
        prog_layout.addLayout(status_row)

        self.progress_bar = QProgressBar()
        self.progress_bar.setRange(0, 100)
        self.progress_bar.setValue(0)
        prog_layout.addWidget(self.progress_bar)
        main_layout.addLayout(prog_layout)

        # Live Games Table
        self.table = QTableWidget()
        self.table.setColumnCount(6)
        self.table.setHorizontalHeaderLabels(["#", "Puzzle ID", "Game ID", "Rating", "Plies", "Result"])
        header = self.table.horizontalHeader()
        header.setSectionResizeMode(0, QHeaderView.ResizeToContents)
        header.setSectionResizeMode(1, QHeaderView.ResizeToContents)
        header.setSectionResizeMode(2, QHeaderView.ResizeToContents)
        header.setSectionResizeMode(3, QHeaderView.ResizeToContents)
        header.setSectionResizeMode(4, QHeaderView.ResizeToContents)
        header.setSectionResizeMode(5, QHeaderView.Stretch)
        self.table.verticalHeader().setVisible(False)
        self.table.setEditTriggers(QTableWidget.NoEditTriggers)
        self.table.setSelectionBehavior(QTableWidget.SelectRows)
        main_layout.addWidget(self.table, stretch=1)

        # Control Buttons
        btn_layout = QHBoxLayout()
        btn_layout.setSpacing(12)

        self.start_btn = QPushButton("▶  Start Download")
        self.start_btn.clicked.connect(self.start_download)
        btn_layout.addWidget(self.start_btn)

        self.pause_btn = QPushButton("⏸  Pause")
        self.pause_btn.setObjectName("pauseBtn")
        self.pause_btn.setEnabled(False)
        self.pause_btn.clicked.connect(self.toggle_pause)
        btn_layout.addWidget(self.pause_btn)

        self.stop_btn = QPushButton("⏹  Stop & Save")
        self.stop_btn.setObjectName("stopBtn")
        self.stop_btn.setEnabled(False)
        self.stop_btn.clicked.connect(self.stop_download)
        btn_layout.addWidget(self.stop_btn)

        main_layout.addLayout(btn_layout)

        # Timer for live UI metrics
        self.ui_timer = QTimer()
        self.ui_timer.setInterval(1000)
        self.ui_timer.timeout.connect(self.update_time_metrics)

    def create_metric_card(self, title, initial_value, color):
        frame = QFrame()
        frame.setProperty("class", "card")
        layout = QVBoxLayout(frame)
        layout.setContentsMargins(8, 8, 8, 8)
        layout.setSpacing(4)

        t_lbl = QLabel(title)
        t_lbl.setStyleSheet("color: #8b949e; font-size: 11px; font-weight: bold; text-transform: uppercase;")
        v_lbl = QLabel(initial_value)
        v_lbl.setStyleSheet(f"color: {color}; font-size: 19px; font-weight: bold;")
        v_lbl.setObjectName("val")

        layout.addWidget(t_lbl)
        layout.addWidget(v_lbl)
        return frame

    def set_card_value(self, card_widget, text):
        v = card_widget.findChild(QLabel, "val")
        if v:
            v.setText(text)

    def update_target_est(self, count):
        approx_kb = count * 0.20  # ~200 bytes per game
        if approx_kb > 1024:
            self.est_size_label.setText(f"≈ {approx_kb / 1024:.1f} MB (.lgdb)")
        else:
            self.est_size_label.setText(f"≈ {approx_kb:.0f} KB (.lgdb)")

    def browse_source_db(self):
        f, _ = QFileDialog.getOpenFileName(self, "Select Puzzle Database", "", "LPDB Files (*.lpdb);;All Files (*)")
        if f:
            self.source_db_edit.setText(f)

    def browse_output_file(self):
        f, _ = QFileDialog.getSaveFileName(self, "Save Game Database As", "", "LGDB Files (*.lgdb);;All Files (*)")
        if f:
            self.output_edit.setText(f)

    def start_download(self):
        bin_path = get_binary_path()
        if not os.path.exists(bin_path):
            self.status_label.setText("Error: lpdb.exe binary not found. Build it with 'cargo build --release' first.")
            return

        source_db = self.source_db_edit.text().strip()
        if not os.path.exists(source_db):
            self.status_label.setText(f"Error: Source database '{source_db}' not found.")
            return

        cmd = [
            bin_path,
            "fetch-games",
            "--from-db", source_db,
            "--min-rating", str(self.min_rating_spin.value()),
            "--max-rating", str(self.max_rating_spin.value()),
            "--limit", str(self.target_games_spin.value()),
            "--output", self.output_edit.text().strip(),
            "--pause-every", str(self.pause_every_spin.value()),
            "--pause-seconds", str(self.pause_sec_spin.value()),
            "--json-progress",
            "--yes",
        ]

        self.table.setRowCount(0)
        self.progress_bar.setValue(0)
        self.start_time = time.time()
        self.last_count = 0
        self.last_time = self.start_time

        self.start_btn.setEnabled(False)
        self.pause_btn.setEnabled(True)
        self.pause_btn.setText("⏸  Pause")
        self.stop_btn.setEnabled(True)
        self.status_label.setText("Status: Downloading games from Lichess...")
        self.status_label.setStyleSheet("font-weight: bold; color: #3fb950;")

        self.worker = DownloadWorker(cmd)
        self.worker.progress_signal.connect(self.on_progress)
        self.worker.cooldown_signal.connect(self.on_cooldown)
        self.worker.complete_signal.connect(self.on_complete)
        self.worker.log_signal.connect(self.on_log)
        self.worker.error_signal.connect(self.on_error)
        self.worker.finished.connect(self.on_worker_finished)
        self.worker.start()

        self.ui_timer.start()

    def toggle_pause(self):
        if self.worker:
            is_paused = self.worker.toggle_pause()
            if is_paused:
                self.pause_btn.setText("▶  Resume")
                self.status_label.setText("Status: Paused by user")
                self.status_label.setStyleSheet("font-weight: bold; color: #d29922;")
            else:
                self.pause_btn.setText("⏸  Pause")
                self.status_label.setText("Status: Downloading games...")
                self.status_label.setStyleSheet("font-weight: bold; color: #3fb950;")

    def stop_download(self):
        if self.worker:
            self.status_label.setText("Status: Stopping and saving database...")
            self.worker.stop()

    def on_progress(self, data):
        idx = data.get("index", 0)
        total = data.get("total", 1)
        succeeded = data.get("succeeded", 0)
        plies = data.get("total_plies", 0)
        pid = data.get("puzzle_id", "")
        gid = data.get("game_id", "")

        pct = int((idx / max(total, 1)) * 100)
        self.progress_bar.setValue(pct)

        self.set_card_value(self.card_downloaded, f"{succeeded:,}")
        self.set_card_value(self.card_plies, f"{plies:,}")

        approx_kb = succeeded * 0.20
        if approx_kb > 1024:
            self.set_card_value(self.card_size, f"{approx_kb / 1024:.2f} MB")
        else:
            self.set_card_value(self.card_size, f"{approx_kb:.1f} KB")

        # Add to table
        row = self.table.rowCount()
        self.table.insertRow(row)
        self.table.setItem(row, 0, QTableWidgetItem(str(idx)))
        self.table.setItem(row, 1, QTableWidgetItem(pid))
        self.table.setItem(row, 2, QTableWidgetItem(gid))
        self.table.setItem(row, 3, QTableWidgetItem(str(self.min_rating_spin.value()) + "–" + str(self.max_rating_spin.value())))
        self.table.setItem(row, 4, QTableWidgetItem(str(plies // max(succeeded, 1))))
        self.table.setItem(row, 5, QTableWidgetItem("Saved ✓"))
        self.table.scrollToBottom()

    def on_cooldown(self, data):
        dur = data.get("duration_sec", 2.0)
        msg = data.get("message", f"Cooling down for {dur:.1f}s...")
        self.status_label.setText(f"Status: {msg}")
        self.status_label.setStyleSheet("font-weight: bold; color: #58a6ff;")

    def on_complete(self, data):
        self.status_label.setText("Status: ✓ Download completed successfully!")
        self.status_label.setStyleSheet("font-weight: bold; color: #3fb950;")
        succeeded = data.get("succeeded", 0)
        file_size = data.get("file_size", 0)
        self.set_card_value(self.card_downloaded, f"{succeeded:,}")
        self.set_card_value(self.card_size, f"{file_size / (1024 * 1024):.2f} MB")
        self.progress_bar.setValue(100)

    def on_log(self, text):
        if "Cooling down" in text:
            self.status_label.setText(f"Status: {text}")

    def on_error(self, err):
        self.status_label.setText(f"Error: {err}")
        self.status_label.setStyleSheet("font-weight: bold; color: #f85149;")

    def on_worker_finished(self):
        self.start_btn.setEnabled(True)
        self.pause_btn.setEnabled(False)
        self.stop_btn.setEnabled(False)
        self.ui_timer.stop()
        if "completed" not in self.status_label.text().lower():
            self.status_label.setText("Status: Stopped / Ready")
            self.status_label.setStyleSheet("font-weight: bold; color: #8b949e;")

    def update_time_metrics(self):
        if not self.start_time:
            return
        elapsed = time.time() - self.start_time
        elapsed_str = time.strftime("%H:%M:%S", time.gmtime(elapsed))

        current_count = int(self.card_downloaded.findChild(QLabel, "val").text().replace(',', ''))
        now = time.time()
        dt = now - self.last_time
        if dt >= 1.0:
            speed = (current_count - self.last_count) / dt
            self.set_card_value(self.card_speed, f"{speed:.1f} g/s")
            self.last_count = current_count
            self.last_time = now

            target = self.target_games_spin.value()
            rem = max(0, target - current_count)
            if speed > 0:
                eta_sec = rem / speed
                eta_str = time.strftime("%H:%M:%S", time.gmtime(eta_sec))
            else:
                eta_str = "--:--:--"
            self.time_label.setText(f"Elapsed: {elapsed_str} | ETA: {eta_str}")


def main():
    app = QApplication(sys.argv)
    window = MainWindow()
    window.show()
    sys.exit(app.exec_())


if __name__ == "__main__":
    main()
