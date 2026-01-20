set shell := ["zsh", "-uc"]
export RUST_LOG := "{{ env_var_or_default('RUST_LOG', 'info') }}"

default: run

front *args="":
    cargo run -- {{args}}

run *args="":
    cargo run -- {{args}}

transcribe file *args="":
    cargo run -- transcribe --input {{file}} {{args}}

models:
    cargo run -- models
