PREFIX ?= /usr/local
DESTDIR ?=
CARGO ?= cargo
# Where `make install` compiles the xterm-ultty terminfo entry. ncurses on
# most distributions searches /usr/share/terminfo and ~/.terminfo but not
# /usr/local/share/terminfo; set TERMINFO_DIR to one it searches (or add
# this directory to TERMINFO_DIRS) to use terminal.term = "xterm-ultty".
TERMINFO_DIR ?= $(PREFIX)/share/terminfo

.PHONY: build release install uninstall test check fmt lint examples clean dist

build:
	$(CARGO) build --workspace

release:
	$(CARGO) build --workspace --release

install: release
	install -Dm755 target/release/ultty $(DESTDIR)$(PREFIX)/bin/ultty
	install -Dm644 dist/ultty.desktop $(DESTDIR)$(PREFIX)/share/applications/ultty.desktop
	@if command -v tic >/dev/null 2>&1; then \
		mkdir -p "$(DESTDIR)$(TERMINFO_DIR)" && \
		tic -x -o "$(DESTDIR)$(TERMINFO_DIR)" dist/terminfo/ultty.terminfo && \
		echo "installed the xterm-ultty terminfo entry in $(TERMINFO_DIR)"; \
	else \
		echo "tic not found; skipping the xterm-ultty terminfo entry"; \
	fi

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/ultty
	rm -f $(DESTDIR)$(PREFIX)/share/applications/ultty.desktop
	rm -f $(DESTDIR)$(TERMINFO_DIR)/x/xterm-ultty

test:
	$(CARGO) test --workspace

check:
	$(CARGO) check --workspace

fmt:
	$(CARGO) fmt --all

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

examples:
	$(CARGO) build --workspace --examples

clean:
	$(CARGO) clean

dist: release
	@mkdir -p dist/pkg
	@cp target/release/ultty dist/pkg/
	@cp dist/ultty.desktop dist/pkg/
	@cp dist/terminfo/ultty.terminfo dist/pkg/
	@echo "Distribution files in dist/pkg/"
