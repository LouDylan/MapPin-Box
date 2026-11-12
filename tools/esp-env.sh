# Shared shell environment for the Xtensa Rust builds.
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
waypin_esp_env=${WAYPIN_ESP_ENV:-$HOME/export-esp.sh}
if [ -r "$waypin_esp_env" ]; then
    . "$waypin_esp_env"
fi
