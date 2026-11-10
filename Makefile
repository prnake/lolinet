# One entry point for building and checking everything in this repository.
# The checks mirror what CI runs (.github/workflows/{build,desk,web}.yml), so they can run locally.
#
#   make lolinet        lolinet for this machine → .build/out/lolinet-<os>-<arch>.tar.gz
#   make desk           lolidesk-host and lolidesk → desk/target/release/ (--version prints git describe, as in CI)
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

.PHONY: all lolinet desk web check check-web check-scripts check-desk check-sni clean

all: lolinet

lolinet:
	sh lolinet/build.sh

desk:
	cd desk && LOLIDESK_VERSION=$$(git describe --always --dirty 2>/dev/null || echo dev) cargo build --locked --release --workspace

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

check-desk:
	cd desk && cargo fmt --all --check
	cd desk && cargo clippy --locked --workspace --all-targets -- -D warnings
	cd desk && cargo test --locked --workspace

check-sni:
	@test -n "$(CORE)" || { echo "usage: make check-sni CORE=path/to/easytier-core" >&2; exit 2; }
	$(PYTHON) easytier/check_sni.py "$(CORE)"

clean:
	rm -rf .build web/dist web/dist-probe
