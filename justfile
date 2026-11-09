default:
    just --list

build:
    cargo build --release --locked

run *args:
    cargo run --release --locked -- {{args}}

format:
    cargo fmt --all

test:
    cargo test --locked

lint:
    cargo clippy --all-targets --locked -- -D warnings

check:
    cargo fmt --all -- --check
    cargo clippy --all-targets --locked -- -D warnings
    cargo test --locked

benchmark *args:
    cargo run --release --locked -- --benchmark {{args}}
