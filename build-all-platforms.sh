#!/bin/bash

# Define output directory
DIST_DIR="dist"
mkdir -p "$DIST_DIR"

# Define target platforms
# "windows/amd64" is not fully supported
PLATFORMS=(
    "darwin/amd64"
    "darwin/arm64"
    "linux/amd64"
)

# Build for each platform
for PLATFORM in "${PLATFORMS[@]}"; do
    OS=$(echo "$PLATFORM" | cut -d'/' -f1)
    ARCH=$(echo "$PLATFORM" | cut -d'/' -f2)

    OUTPUT_NAME="dbt-language-server-${OS}-${ARCH}"
    if [ "$OS" == "windows" ]; then
        OUTPUT_NAME+=".exe"
    fi

    echo "Building for $OS/$ARCH..."

    GOOS=$OS GOARCH=$ARCH go build -o "$DIST_DIR/$OUTPUT_NAME" .

    if [ $? -ne 0 ]; then
        echo "Failed to build for $OS/$ARCH"
        exit 1
    fi
done

echo "All builds completed successfully!"

