PREFIX ?= /usr/local
DESTDIR ?=
CARGO ?= cargo

.PHONY: build release install uninstall test check fmt lint examples clean dist

build:
	$(CARGO) build --workspace

release:
	$(CARGO) build --workspace --release

install: release
	install -Dm755 target/release/ultty $(DESTDIR)$(PREFIX)/bin/ultty
	install -Dm644 dist/ultty.desktop $(DESTDIR)$(PREFIX)/share/applications/ultty.desktop

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/ultty
	rm -f $(DESTDIR)$(PREFIX)/share/applications/ultty.desktop

test:
	$(CARGO) test --workspace

check:
	$(CARGO) check --workspace

fmt:
	$(CARGO) fmt --all

lint:
	$(CARGO) clippy --workspace -- -D warnings

examples:
	$(CARGO) build --workspace --examples

clean:
	$(CARGO) clean

dist: release
	@mkdir -p dist/pkg
	@cp target/release/ultty dist/pkg/
	@cp dist/ultty.desktop dist/pkg/
	@echo "Distribution files in dist/pkg/"
