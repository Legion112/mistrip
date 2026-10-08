# mistrip — native Linux control for the Xiaomi Smart Lightstrip Pro.
#
# Run `make` or `make help` for the available targets.

CARGO      ?= cargo
PYTHON     ?= python3
BIN        := mistrip
DEBUG_BIN  := target/debug/$(BIN)
RELEASE_BIN:= target/release/$(BIN)
CONFIG     := $(HOME)/.config/mistrip/devices.json
EXTRACTOR  ?= $(HOME)/github/Xiaomi-cloud-tokens-extractor/token_extractor.py

# Colour, only when stdout is a terminal.
BOLD := $(shell test -t 1 && tput bold 2>/dev/null)
DIM  := $(shell test -t 1 && tput dim 2>/dev/null)
OFF  := $(shell test -t 1 && tput sgr0 2>/dev/null)

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help
	@echo "$(BOLD)mistrip$(OFF) — Xiaomi Smart Lightstrip Pro control"
	@echo
	@grep -hE '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  $(BOLD)%-14s$(OFF) %s\n", $$1, $$2}'
	@echo
	@echo "$(DIM)Variables: CARGO, PYTHON, EXTRACTOR, COLOR, LEVEL, MODE, SEGMENTS$(OFF)"

# ---------------------------------------------------------------- build

.PHONY: build
build: ## Debug build
	$(CARGO) build

.PHONY: release
release: ## Optimised build
	$(CARGO) build --release

.PHONY: install
install: ## Install both binaries into ~/.cargo/bin
	$(CARGO) install --path . --locked

.PHONY: clean
clean: ## Remove build artefacts
	$(CARGO) clean

# ---------------------------------------------------------------- desktop

BINDIR    ?= $(HOME)/.cargo/bin
DATADIR   ?= $(HOME)/.local/share
APPDIR    := $(DATADIR)/applications
ICONDIR   := $(DATADIR)/icons/hicolor

.PHONY: install-desktop
install-desktop: ## Install the icon and menu entry for the GUI
	@test -x "$(BINDIR)/mistrip-gui" \
		|| { echo "$(BINDIR)/mistrip-gui not found — run 'make install' first"; exit 1; }
	install -Dm644 assets/mistrip.svg     "$(ICONDIR)/scalable/apps/mistrip.svg"
	install -Dm644 assets/icon-64.png     "$(ICONDIR)/64x64/apps/mistrip.png"
	install -Dm644 assets/icon-128.png    "$(ICONDIR)/128x128/apps/mistrip.png"
	install -Dm644 assets/icon-256.png    "$(ICONDIR)/256x256/apps/mistrip.png"
	@mkdir -p "$(APPDIR)"
	sed 's|@BINDIR@|$(BINDIR)|' assets/mistrip.desktop.in > "$(APPDIR)/mistrip.desktop"
	@chmod 644 "$(APPDIR)/mistrip.desktop"
	-update-desktop-database "$(APPDIR)" 2>/dev/null
	-gtk-update-icon-cache -f -t "$(ICONDIR)" 2>/dev/null
	@echo "installed: $(APPDIR)/mistrip.desktop -> $(BINDIR)/mistrip-gui"

.PHONY: uninstall-desktop
uninstall-desktop: ## Remove the icon and menu entry
	rm -f "$(APPDIR)/mistrip.desktop"
	rm -f "$(ICONDIR)/scalable/apps/mistrip.svg"
	rm -f "$(ICONDIR)/64x64/apps/mistrip.png"
	rm -f "$(ICONDIR)/128x128/apps/mistrip.png"
	rm -f "$(ICONDIR)/256x256/apps/mistrip.png"
	-update-desktop-database "$(APPDIR)" 2>/dev/null
	-gtk-update-icon-cache -f -t "$(ICONDIR)" 2>/dev/null
	@echo "removed the desktop entry and icons"

.PHONY: gui
gui: build ## Run the GUI from the build tree
	./target/debug/mistrip-gui

# ---------------------------------------------------------------- quality

.PHONY: test
test: ## Run the unit tests
	$(CARGO) test

.PHONY: fmt
fmt: ## Format the source
	$(CARGO) fmt

.PHONY: fmt-check
fmt-check: ## Fail if the source is not formatted
	$(CARGO) fmt --check

.PHONY: clippy
clippy: ## Lint, warnings are errors
	$(CARGO) clippy --all-targets -- -D warnings

.PHONY: audit
audit: ## Check dependencies for advisories (needs cargo-audit)
	@command -v cargo-audit >/dev/null 2>&1 \
		|| { echo "cargo-audit not installed: cargo install cargo-audit"; exit 1; }
	$(CARGO) audit

.PHONY: check
check: fmt-check clippy test ## Everything CI should run

# ---------------------------------------------------------------- device

.PHONY: token
token: ## Fetch the device token from Mi Cloud into the config file
	@test -f "$(EXTRACTOR)" \
		|| { echo "extractor not found at $(EXTRACTOR)"; \
		     echo "clone https://github.com/PiotrMachowski/Xiaomi-cloud-tokens-extractor"; \
		     echo "or override: make token EXTRACTOR=/path/to/token_extractor.py"; exit 1; }
	@mkdir -p $(dir $(CONFIG)) && chmod 700 $(dir $(CONFIG))
	$(PYTHON) "$(EXTRACTOR)" -o "$(CONFIG)"
	@chmod 600 "$(CONFIG)" && echo "wrote $(CONFIG) (mode 0600)"

.PHONY: config-check
config-check: ## Verify the credentials file exists with safe permissions
	@test -f "$(CONFIG)" || { echo "missing $(CONFIG) — run 'make token'"; exit 1; }
	@mode=$$(stat -c %a "$(CONFIG)"); \
		test "$$mode" = "600" \
		|| { echo "$(CONFIG) is mode $$mode, should be 600: chmod 600 $(CONFIG)"; exit 1; }
	@echo "$(CONFIG) ok (mode 600)"

.PHONY: devices
devices: build config-check ## List every device in the credentials file
	./$(DEBUG_BIN) devices

.PHONY: status
status: build config-check ## Show the strip's current state
	./$(DEBUG_BIN) status

.PHONY: probe
probe: config-check ## Read-only property dump via the Python reference tool
	$(PYTHON) tools/probe-strip.py

# Ad-hoc control, e.g. `make color COLOR=FF0000`
COLOR    ?= FF0000
LEVEL    ?= 60
MODE     ?= 3
SEGMENTS ?= 0:FF0000 1:00FF00 2:0000FF

.PHONY: on
on: build ## Power on
	./$(DEBUG_BIN) on

.PHONY: off
off: build ## Power off
	./$(DEBUG_BIN) off

.PHONY: color
color: build ## Set a colour, e.g. make color COLOR=00FF00
	./$(DEBUG_BIN) color $(COLOR)

.PHONY: brightness
brightness: build ## Set brightness, e.g. make brightness LEVEL=40
	./$(DEBUG_BIN) brightness $(LEVEL)

.PHONY: mode
mode: build ## Select a scene, e.g. make mode MODE=5
	./$(DEBUG_BIN) mode $(MODE)

.PHONY: segments
segments: build ## Paint segments, e.g. make segments SEGMENTS="0:FF0000 1:0000FF"
	./$(DEBUG_BIN) segments $(SEGMENTS)
