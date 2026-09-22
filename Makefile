SHELL := /bin/bash

PROJECT_NAME := token-usage-insights
PORT ?= 3003

CARGO := cargo
SERVICE_TEMPLATE := shell/token-usage-insights.service
SERVICE_NAME := token-usage-insights.service
SERVICE_FILE := /etc/systemd/system/$(SERVICE_NAME)
RELEASE_BIN := target/release/$(PROJECT_NAME)

.PHONY: help run dev run-release build build-release test test-scripts fmt clippy check lint all clean \
	service-file install-service uninstall-service enable-service disable-service \
	start-service stop-service restart-service status

help:
	@echo "Token 戰情室 Makefile"
	@echo
	@echo "常用指令："
	@echo "  make run             以 Release 版本啟動本機伺服器（含建置，預設 PORT=$(PORT)）"
	@echo "  make dev             以 Debug 版本啟動（快速迭代用，預設 PORT=$(PORT)）"
	@echo "  make run-release     等同 run"
	@echo "  make build           建置 Debug 版本"
	@echo "  make build-release   建置 Release 版本"
	@echo "  make test            執行 Rust 測試"
	@echo "  make test-scripts    執行安裝腳本的 systemd 單元測試（不需要 systemd）"
	@echo "  make fmt             套用 Rust formatting"
	@echo "  make clippy          執行 clippy 全量檢查"
	@echo "  make check           執行 cargo check --all-targets --all-features"
	@echo "  make lint            先執行 fmt 後執行 clippy"
	@echo "  make all             依序執行 fmt、check、test、build-release"
	@echo "  make clean           清除建置快取"
	@echo "  make service-file    產生 service 檔（輸出到標準輸出）"
	@echo "  make install-service 安裝並重新載入 systemd service（需要 sudo）"
	@echo "  make uninstall-service 移除 systemd service 並重新載入（需要 sudo）"
	@echo "  make enable-service  啟用開機自動啟動（需要 sudo）"
	@echo "  make disable-service 停用開機自動啟動（需要 sudo）"
	@echo "  make start-service   啟動 service（需要 sudo）"
	@echo "  make stop-service    停止 service（需要 sudo）"
	@echo "  make restart-service 重啟 service（需要 sudo）"
	@echo "  make status          查詢 service 狀態（需要 sudo）"

run: build-release
	PORT=$(PORT) $(RELEASE_BIN)

dev:
	PORT=$(PORT) $(CARGO) run --bin $(PROJECT_NAME)

run-release: run

build:
	$(CARGO) build

build-release:
	$(CARGO) build --release

test:
	$(CARGO) test

test-scripts:
	bash tests/install-systemd.test.sh

fmt:
	$(CARGO) fmt

clippy:
	$(CARGO) clippy --all-targets --all-features

check:
	$(CARGO) check --all-targets --all-features

lint: fmt clippy

all: fmt check test build-release

clean:
	$(CARGO) clean

service-file:
	@sed "s|<PROJECT_DIR>|$(CURDIR)|g" $(SERVICE_TEMPLATE) | \
		sed "s|^Environment=PORT=.*|Environment=PORT=$(PORT)|"

install-service:
	@if [ "$$(uname -s)" != "Linux" ]; then \
		echo "systemd service 安裝僅支援 Linux。"; \
		exit 1; \
	fi
	@tmp="$$(mktemp)"; \
	trap 'rm -f "$$tmp"' EXIT; \
	$(CARGO) build --release && \
		sed "s|<PROJECT_DIR>|$(CURDIR)|g" $(SERVICE_TEMPLATE) | \
		sed "s|^Environment=PORT=.*|Environment=PORT=$(PORT)|" > "$$tmp" && \
		sudo install -m 644 "$$tmp" $(SERVICE_FILE) && \
		sudo systemctl daemon-reload

uninstall-service:
	@if [ "$$(uname -s)" != "Linux" ]; then \
		echo "systemd service 移除僅支援 Linux。"; \
		exit 1; \
	fi
	@sudo systemctl stop $(SERVICE_NAME) || true
	@sudo rm -f $(SERVICE_FILE)
	@sudo systemctl daemon-reload

enable-service:
	@if [ "$$(uname -s)" != "Linux" ]; then \
		echo "systemd service 啟用僅支援 Linux。"; \
		exit 1; \
	fi
	@sudo systemctl enable $(SERVICE_NAME)

disable-service:
	@if [ "$$(uname -s)" != "Linux" ]; then \
		echo "systemd service 停用僅支援 Linux。"; \
		exit 1; \
	fi
	@sudo systemctl disable $(SERVICE_NAME)

start-service:
	@if [ "$$(uname -s)" != "Linux" ]; then \
		echo "systemd service 啟動僅支援 Linux。"; \
		exit 1; \
	fi
	@sudo systemctl start $(SERVICE_NAME)

stop-service:
	@if [ "$$(uname -s)" != "Linux" ]; then \
		echo "systemd service 停止僅支援 Linux。"; \
		exit 1; \
	fi
	@sudo systemctl stop $(SERVICE_NAME)

restart-service:
	@if [ "$$(uname -s)" != "Linux" ]; then \
		echo "systemd service 重啟僅支援 Linux。"; \
		exit 1; \
	fi
	@sudo systemctl restart $(SERVICE_NAME)

status:
	@if [ "$$(uname -s)" != "Linux" ]; then \
		echo "systemd service 查詢僅支援 Linux。"; \
		exit 1; \
	fi
	@sudo systemctl status $(SERVICE_NAME)
