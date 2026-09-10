GO_FILES := $(shell find cmd internal -name '*.go' -type f | LC_ALL=C sort)

.PHONY: build test vet fmt-check check

build:
	go build ./cmd/slurp

test:
	go test -race ./...

vet:
	go vet ./...

fmt-check:
	@test -z "$$(gofmt -l $(GO_FILES))" || { gofmt -l $(GO_FILES); exit 1; }

check: fmt-check test vet build
