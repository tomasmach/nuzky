Unsigned preview builds. Linux: x86_64 Ubuntu 24.04 or newer; macOS: Apple Silicon, macOS 14 or newer; Windows: x64, WebView2 required (installer downloads it if missing).

AppImage includes FFmpeg libraries. The Ubuntu .deb uses distribution FFmpeg packages. macOS and Windows include FFmpeg shared libraries. macOS is ad-hoc signed, not notarized; Windows is not Authenticode signed.

Before publishing this draft, test import, playback with sound, export and captions on each OS. Follow docs/BUILDING.md to include the corresponding source and licences of all redistributed libraries. Build logs and build-info files are provenance, not a substitute for corresponding source.
