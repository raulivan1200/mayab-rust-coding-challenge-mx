APP=mayab-arbitrage

.PHONY: run test check e2e contrast smoke release-check build docker

run:
	cargo run

test:
	cargo test --workspace

check:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets --locked -- -D warnings
	cargo test --workspace --all-targets --locked
	@for file in $$(find internal/webui/web -name '*.js' -type f | sort); do \
		echo "node --check $$file"; \
		node --check "$$file"; \
	done
	node scripts/check-webui-contrast.mjs

e2e:
	npm run test:e2e

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
