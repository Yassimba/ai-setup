.PHONY: setup web embed-static test build serve

setup:
	npm --prefix web ci

web:
	npm --prefix web run build

embed-static: web
	rm -rf internal/server/static/assets
	cp web/dist/index.html internal/server/static/index.html
	cp -R web/dist/assets internal/server/static/assets
	rm -f internal/server/static/assets/*.map

test:
	go test ./...
	npm --prefix web run build

build: embed-static
	mkdir -p bin
	go build -o bin/mindwalk ./cmd/mindwalk

serve: web
	go run ./cmd/mindwalk serve --port 8765 --dev
