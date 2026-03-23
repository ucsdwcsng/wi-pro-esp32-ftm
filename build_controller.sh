#!/bin/bash

RELEASE_MODE=""
TARGET=""

while [[ $# -gt 0 ]]; do
    case $1 in
        -r|--release)
            RELEASE_MODE="--release"
            shift
            ;;
        -t|--target)
            TARGET="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [-r|--release] [-t|--target <target-triple>]"
            echo "Example: $0 --release --target armv7-unknown-linux-gnueabihf"
            exit 1
            ;;
    esac
done

if [ -n "$RELEASE_MODE" ]; then
    BUILD_TYPE="release"
else
    BUILD_TYPE="debug"
fi

if [ -n "$TARGET" ]; then
    TARGET_FLAG="--target $TARGET"
    TARGET_PATH="$TARGET/$BUILD_TYPE"
else
    TARGET_FLAG=""
    TARGET_PATH="$BUILD_TYPE"
fi

(
    SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
    cd "$SCRIPT_DIR/controller"
    cargo build $RELEASE_MODE $TARGET_FLAG

    if [ $? -ne 0 ]; then
        exit 1
    fi
    
    if [ -n "$TARGET" ]; then
        echo "Built for target: $TARGET"
    else
	echo "Built for local arch"
	cp "$SCRIPT_DIR/controller/target/$TARGET_PATH/controller" "$SCRIPT_DIR/controller/"
	cp "$SCRIPT_DIR/controller/target/$TARGET_PATH/server" "$SCRIPT_DIR/controller/"
    fi
)

if [ $? -eq 0 ]; then
    echo "Build successful!"
    if [ -n "$TARGET" ]; then
        echo "Built for target: $TARGET"
    fi
    exit 0
else
    echo "Build failed. See output above."
    exit 1
fi
