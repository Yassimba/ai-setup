# herdr-review dev tasks — run `just <task>` (https://github.com/casey/just)

# default: list tasks
default:
    @just --list

# format the code
fmt:
    cargo fmt --all

# check formatting (CI parity)
fmt-check:
    cargo fmt --all --check

# lint with clippy, warnings as errors (CI parity)
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# run the test suite
test:
    cargo test --all-features

# build (debug)
build:
    cargo build

# run the sidebar in the current repo
run:
    cargo run

# build release and install the binary into bin/ for `herdr plugin link`
install:
    cargo build --release
    mkdir -p bin
    # Replace with a fresh inode, then ad-hoc re-sign: macOS SIGKILLs a binary whose signature
    # an in-place overwrite invalidated, so a plain `cp` over the old binary makes the pane die
    # at launch on Apple Silicon. No-op on Linux.
    rm -f bin/herdr-reviewr
    cp target/release/herdr-reviewr bin/herdr-reviewr
    [ "$(uname)" = "Darwin" ] && codesign --force --sign - bin/herdr-reviewr || true

# everything CI runs, locally
ci: fmt-check lint test
    cargo build --release
