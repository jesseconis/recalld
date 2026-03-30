name := `cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].name'`
version := `cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'`

# Build release binary
build:
	@echo "Building 👷  {{name}} v{{version}}..."
	mkdir -p dist
	cargo build --release
	cp target/release/{{name}} dist/{{name}}-v{{version}}
	@echo "Done ✅"

test:
	cargo test

prepare-release level='patch':
    cargo-release release --execute {{level}}

gh-release level='patch': (prepare-release level)
    #!/usr/bin/env bash
    just build
    git fetch --tags
    VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c "import sys,json; print(json.load(sys.stdin)['packages'][0]['version'])")
    gh release create "v${VERSION}" \
        "./dist/{{name}}-${VERSION}" \
        --generate-notes

# Install binary and systemd service for current user
install: build
	install -Dm755 target/release/{{name}} ~/.cargo/bin/{{name}}
	install -Dm644 recalld.service ~/.config/systemd/user/recalld.service
	systemctl --user daemon-reload
	@echo "Installed ✅ — run 'systemctl --user enable --now recalld' to start"

# Uninstall binary and service
uninstall:
	systemctl --user stop recalld || true
	systemctl --user disable recalld || true
	rm -f ~/.cargo/bin/{{name}}
	rm -f ~/.config/systemd/user/recalld.service
	systemctl --user daemon-reload
	@echo "Uninstalled ✅"
