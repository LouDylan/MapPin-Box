# Shared shell environment for the Xtensa Rust builds.
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
mappin_esp_env=${MAPPIN_ESP_ENV:-$HOME/export-esp.sh}
if [ -r "$mappin_esp_env" ]; then
    . "$mappin_esp_env"
fi
