# One entry point for building and checking everything in this repository.
# The checks mirror what CI runs (.github/workflows/{build,desk,web}.yml), so they can run locally.
#
#   make lolinet        lolinet for this machine → .build/out/lolinet-<os>-<arch>.tar.gz
#   make desk           lolidesk (host and client in one) → desk/target/release/ (--version prints git describe, as in CI), libghostty-vt → desk/.ghostty/
#   make desk-app       desktop app (GPUI): Lolidesk.app on macOS, the Linux package layout on Linux → desk/app-out/
#   make web            local console (web/dist) and public probe page (web/dist-probe)
#   make check          web + scripts + desk checks
#   make check-sni CORE=path/to/easytier-core   prove the WSS SNI patch works on a built binary
#   make clean          remove .build/ and the web build output

# Recipes use paths relative to the repository root; refuse to run anywhere else
# (make -C path/to/lolinet works).
ifeq ($(wildcard lolinet/build.sh),)
  $(error run make at the lolinet repository root (make -C path/to/lolinet), not in $(CURDIR))
endif

PYTHON ?= python3
PY_SOURCES = join/deploy.py lolinet/release.py easytier/check_sni.py $(wildcard probe/*.py scripts/*.py web/agent/*.py)

.PHONY: all lolinet desk desk-app web check check-web check-scripts check-desk check-sni clean

all: lolinet

lolinet:
	sh lolinet/build.sh

# The pinned libghostty-vt for this machine (desk/ghostty/libghostty-vt.json), which the host, the app and the tests load.
GHOSTTY_PLATFORM := $(shell uname -s | tr A-Z a-z | sed 's/darwin/darwin/')-$(shell uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')
GHOSTTY_LIB := $(CURDIR)/desk/.ghostty/libghostty-vt.$(if $(filter darwin-%,$(GHOSTTY_PLATFORM)),dylib,so)
DESK_VERSION = $$(git describe --always --dirty 2>/dev/null || echo dev)

$(GHOSTTY_LIB):
	sh desk/scripts/fetch-ghostty.sh $(GHOSTTY_PLATFORM) desk/.ghostty

desk: $(GHOSTTY_LIB)
	cd desk && LOLIDESK_VERSION=$(DESK_VERSION) cargo build --locked --release --workspace

# mihomo for Lolidesk.app (desk/mihomo/mihomo.json pins version and sha256); lolinet mihomo on uses the app's copy
MIHOMO_BIN := $(CURDIR)/desk/.mihomo/mihomo

$(MIHOMO_BIN):
	sh desk/scripts/fetch-mihomo.sh $(GHOSTTY_PLATFORM) desk/.mihomo

ifneq ($(filter darwin-%,$(GHOSTTY_PLATFORM)),)
desk-app: $(GHOSTTY_LIB) $(MIHOMO_BIN)
	cd desk && cargo build --locked --release -p lolidesk-app -p lolidesk
	MIHOMO=$(MIHOMO_BIN) sh desk/scripts/bundle-app.sh desk/target/release/lolidesk-app desk/target/release/lolidesk $(GHOSTTY_LIB) desk/app-out
else
# Linux: the same layout as the release package (lolidesk, libghostty-vt.so, BUILD-INFO, lolidesk-app,
# lolidesk.desktop, lolidesk.svg) → desk/app-out/lolidesk/. Needs GPUI's system packages (desk/README.md).
desk-app: $(GHOSTTY_LIB)
	cd desk && LOLIDESK_VERSION=$(DESK_VERSION) cargo build --locked --release -p lolidesk-app -p lolidesk
	rm -rf desk/app-out/lolidesk && mkdir -p desk/app-out/lolidesk
	cp desk/target/release/lolidesk $(GHOSTTY_LIB) desk/app-out/lolidesk/
	printf 'lolidesk %s\n' "$(DESK_VERSION)" > desk/app-out/lolidesk/BUILD-INFO
	sh desk/scripts/bundle-linux.sh desk/target/release/lolidesk-app desk/app-out/lolidesk
endif

web/node_modules: web/package-lock.json
	cd web && npm ci
	@touch $@

web: web/node_modules
	cd web && npm run build

check: check-web check-scripts check-desk

# Builds against nodes.yaml (or nodes.example.yaml) and makes sure the public bundle carries no internal names.
check-web: web
	cd web && node scripts/check-probe-bundle.mjs

check-scripts:
	@tmp=$$(mktemp -d) && cp join/worker.js "$$tmp/worker.mjs" && node --check "$$tmp/worker.mjs"; rc=$$?; rm -rf "$$tmp"; exit $$rc
	sh -n join/install.sh
	sh -n lolinet/build.sh
	$(PYTHON) -m py_compile $(PY_SOURCES)

check-desk: $(GHOSTTY_LIB)
	cd desk && cargo fmt --all --check
	cd desk && cargo clippy --locked --workspace --all-targets -- -D warnings
	cd desk && LOLIDESK_GHOSTTY_VT=$(GHOSTTY_LIB) cargo test --locked --workspace

check-sni:
	@test -n "$(CORE)" || { echo "usage: make check-sni CORE=path/to/easytier-core" >&2; exit 2; }
	$(PYTHON) easytier/check_sni.py "$(CORE)"

clean:
	rm -rf .build web/dist web/dist-probe desk/.ghostty desk/app-out
