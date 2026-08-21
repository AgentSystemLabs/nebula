# Dev helpers. `make install` puts the latest release build into ~/.cargo/bin.
# (End users install via install.sh; this is for working from a checkout.)

PREFIX ?= $(HOME)/.cargo/bin
BIN := target/release/nebula

.PHONY: build install kill

build:
	cargo build --release

# The cp+mv two-step is load-bearing on macOS: overwriting the installed
# binary in place reuses its inode, and the kernel's cached code signature
# for that inode no longer matches the new contents — every exec then dies
# with SIGKILL (exit 137). A fresh inode forces signature re-validation.
install: build
	cp $(BIN) $(PREFIX)/nebula.new
	mv $(PREFIX)/nebula.new $(PREFIX)/nebula
	@$(PREFIX)/nebula --version
	@$(PREFIX)/nebula _stale-daemon-note

# Stops every active session — run only when you're ready to cut over.
kill:
	$(PREFIX)/nebula kill
