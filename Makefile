PREFIX ?= /usr/local
DESTDIR ?=
CARGO ?= cargo

.PHONY: build release install uninstall test check fmt lint examples clean

build:
	$(CARGO) build --workspace

release:
	$(CARGO) build --workspace --release

install: release
	install -Dm755 target/release/ultty $(DESTDIR)$(PREFIX)/bin/ultty

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/ultty

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
