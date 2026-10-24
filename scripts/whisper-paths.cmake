# ggml builds its Vulkan shader generator as a separate CMake project nested deep in whisper.cpp's build directory.
# Under C:\Users\<name>\code\nuzky the files MSBuild's file tracker writes there pass 260 characters, which it cannot
# do even with long paths enabled, so those projects get a shorter base. whisper-rs-sys passes CMAKE_PROJECT_INCLUDE
# from the environment to CMake, which reads this file after each project().
set_property(DIRECTORY PROPERTY EP_BASE "${CMAKE_BINARY_DIR}/ep")
