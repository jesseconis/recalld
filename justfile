name := `cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].name'`
version := `cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'`


build-bins:
	@echo "Building 👷  {{name}} v{{version}}..."
	mkdir -p dist
	cargo build --release
	cp target/release/{{name}} dist/{{name}}-{{version}}
	@echo "Done ✅"
