#!/bin/bash

set -e

# Parse command line arguments
RELEASE_MODE=""

# Check if esptool is available
if ! command -v esptool &> /dev/null; then
    echo "Error: esptool not found." >&2
    exit 1
fi

while [[ $# -gt 0 ]]; do
    case $1 in
        -r|--release)
            RELEASE_MODE="--release"
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [-r|--release]"
            exit 1
            ;;
    esac
done

# Determine build type for paths
if [ -n "$RELEASE_MODE" ]; then
    BUILD_TYPE="release"
else
    BUILD_TYPE="debug"
fi

echo "Creating IDFT matrix binfile"
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
python3 $SCRIPT_DIR/scripts/create_idft_mat.py

# Build firmware in a subshell
(
    # More aggressive virtualenv deactivation
    unset VIRTUAL_ENV
    unset PYTHONHOME
    unset VIRTUAL_ENV_PROMPT
    unset _OLD_VIRTUAL_PATH
    unset PYTHONPATH
    # Restore original PATH if it was saved
    if [ -n "$_OLD_VIRTUAL_PATH" ]; then
        export PATH="$_OLD_VIRTUAL_PATH"
    else
        # Remove common virtualenv path patterns
        export PATH=$(echo "$PATH" | tr ':' '\n' | grep -v '/venv/bin' | grep -v '/.venv/bin' | tr '\n' ':' | sed 's/:$//')
    fi
    
    # Force use of system python3
    hash -r  # Clear bash's command cache
    
    echo "Using Python: $(which python3)"
    echo "Python version: $(python3 --version)"

    SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
    cd $SCRIPT_DIR/firmware

    ESP_IDF_VERSION="v5.5"  # Update this to match your .cargo/config.toml if different
    ESP_IDF_PATH="$SCRIPT_DIR/firmware/.embuild/espressif/esp-idf/$ESP_IDF_VERSION"

    echo "==== Step 1: Ensuring ESP-IDF is set up ===="
    
    if [ ! -d "$ESP_IDF_PATH" ]; then
	echo "ESP-IDF not found. Triggering download via cargo..."
	# This will download ESP-IDF without doing a full build
	cargo fetch 2>/dev/null || true
	cargo metadata --format-version=1 >/dev/null 2>&1 || true
	
	echo "Building esp-idf-sys to download ESP-IDF..."
	cargo build -p esp-idf-sys 2>&1 # | grep -E "(Downloading|Compiling esp-idf|Cloning)" || true
	
	sleep 2
	
	if [ ! -d "$ESP_IDF_PATH" ]; then
            echo "ERROR: ESP-IDF still not found at $ESP_IDF_PATH"
        echo "Please run 'cargo build' manually first to set up ESP-IDF"
        exit 1
	fi
    fi

    echo "ESP-IDF found at: $ESP_IDF_PATH"

    echo "==== Step 2: Applying timestamp patch ===="

    cd "$ESP_IDF_PATH"

    # Check if patch is already applied
    if grep -q "rxstart_time_cyc" components/esp_wifi/include/esp_wifi_he_types.h 2>/dev/null; then
        echo "✓ Patch already applied"
    else
        echo "Applying patch..."
        
        # Try to apply the patch
        if git apply --check "$SCRIPT_DIR/firmware/patch/0001-fix-wifi-Expose-Rx-pkt-timstamp-related-calculations.patch"; then
            git apply "$SCRIPT_DIR/firmware/patch/0001-fix-wifi-Expose-Rx-pkt-timstamp-related-calculations.patch"
            echo "✓ Patch applied successfully"
            
            # Mark that we need to rebuild esp-idf-sys
            NEED_CLEAN=1
        else
            echo "ERROR: Patch failed to apply cleanly"
            echo "Current ESP-IDF version: $(git describe --tags 2>/dev/null || echo 'unknown')"
            exit 1
        fi
    fi

    # Verify patch is present
    if ! grep -q "rxstart_time_cyc" components/esp_wifi/include/esp_wifi_he_types.h; then
        echo "ERROR: Patch verification failed"
        exit 1
    fi

    cd "$SCRIPT_DIR/firmware"

    echo "==== Step 3: Building firmware ===="
    
    if [ -n "$RELEASE_MODE" ]; then
        echo "Building in RELEASE mode..."
    else
        echo "Building in DEBUG mode..."
    fi

    # If we just applied the patch, clean esp-idf-sys to regenerate bindings
    if [ -n "$NEED_CLEAN" ]; then
        echo "Cleaning esp-idf-sys to regenerate bindings..."
        cargo clean -p esp-idf-sys
    fi

    echo "Building firmware..."
    cargo build $RELEASE_MODE --target xtensa-esp32s3-espidf

    # Verify the bindings include the new fields
    echo "Verifying patched fields in Rust bindings..."
    if grep -q "rxstart_time_cyc" target/xtensa-esp32s3-espidf/$BUILD_TYPE/build/esp-idf-sys-*/out/bindings.rs 2>/dev/null; then
        echo "✓ Patched fields found in Rust bindings"
    else
        echo "WARNING: Patched fields not found in Rust bindings"
    fi
        echo "==== Step 4: Copying firmware binary ===="

)

if [ $? -eq 0 ]; then
    SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
    cd "$SCRIPT_DIR/firmware"
    FIRMWARE_FOLDER="target/xtensa-esp32s3-espidf/$BUILD_TYPE"
    if [ -f "$FIRMWARE_FOLDER/firmware" ]; then
	mkdir -p "$(dirname "$FIRMWARE_DEST")"
	cp "$FIRMWARE_FOLDER/firmware" "bin/"
	cp "$FIRMWARE_FOLDER/bootloader.bin" "bin/"
	cp "$FIRMWARE_FOLDER/partition-table.bin" "bin/"
	esptool --chip esp32s3 elf2image bin/firmware
        TOOLCHAIN_DIR=$(find "$SCRIPT_DIR/firmware/.embuild/espressif/tools/xtensa-esp-elf" -maxdepth 1 -type d -name "esp-*" | sort -V | tail -1)
	$TOOLCHAIN_DIR/xtensa-esp-elf/bin/xtensa-esp32s3-elf-objdump -dS bin/firmware > bin/firmware.dis
	ls "bin/"
	echo "✓ Firmware copied to $FIRMWARE_DEST"

	ls -lh "bin/"
    else
	echo "ERROR: Firmware binary not found at $FIRMWARE_SRC"
	exit 1
    fi
    echo "Build successful!"
else
    echo "Build failed. See output above."
    exit 1
fi
