#!/usr/bin/env bash
# Installs the build dependencies on Ubuntu 24.04. CI uses this script, so it is the list that is known to work.
set -euo pipefail

sudo apt-get update
sudo apt-get install -y libwebkit2gtk-4.1-dev libasound2-dev clang cmake pkg-config \
  ffmpeg libavcodec-dev libavformat-dev libavutil-dev libswscale-dev \
  libswresample-dev libavfilter-dev libavdevice-dev libx264-dev \
  patchelf libfuse2t64 librsvg2-bin libvulkan1 mesa-vulkan-drivers libvulkan-dev glslc
