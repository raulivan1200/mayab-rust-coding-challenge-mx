APP=mayab-arbitrage

.PHONY: run test check contrast smoke release-check build docker

run:
	cargo run

test:
	cargo test

check:
	cargo fmt -- --check
	cargo clippy -- -D warnings
	cargo test
	node --check internal/webui/web/app.js
	node scripts/check-webui-contrast.mjs

contrast:
	node scripts/check-webui-contrast.mjs

smoke:
	./scripts/smoke-demo.sh

release-check:
	./scripts/release-check.sh

build:
	cargo build --release
	cp target/release/$(APP) ./$(APP)

docker:
	docker compose up --build
