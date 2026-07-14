.PHONY: build init up up-hosted down restart logs shell test fmt fmt-check clippy admin clean

# Build the app (production) and dev (toolchain) images.
build:
	docker compose build

# First-time setup: build images, then start db + app. Migrations apply automatically on
# app startup.
init: build up

# Start db + app in the background.
up:
	docker compose up -d db app

# Start db + app + the hosted admin dashboard/webhook service (opt-in profile -- self-hosted
# deployments should keep using `make up`).
up-hosted:
	docker compose --profile hosted up -d db app hosted

down:
	docker compose down

restart: down up

logs:
	docker compose logs -f app

# Drop into the dev toolchain container for ad-hoc work.
shell:
	docker compose run --rm dev bash

test:
	docker compose run --rm dev cargo test --workspace

fmt:
	docker compose run --rm dev cargo fmt

fmt-check:
	docker compose run --rm dev cargo fmt --check

clippy:
	docker compose run --rm dev cargo clippy --workspace --all-targets -- -D warnings

# Usage: make admin ARGS="create-tenant demo"
admin:
	docker compose exec app yorishiro-server admin $(ARGS)

# Stops containers and deletes the pgdata volume too (fresh database on next `make init`).
clean:
	docker compose down -v
