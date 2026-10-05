cmake_minimum_required(VERSION 3.16)
project(occ_history_probe CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# OCCT ships a CMake config (OpenCASCADEConfig.cmake) that defines
# OpenCASCADE_INCLUDE_DIR and OpenCASCADE_LIBRARIES.
find_package(OpenCASCADE REQUIRED)

add_executable(occ_history_probe probe.cpp)
target_include_directories(occ_history_probe PRIVATE ${OpenCASCADE_INCLUDE_DIR})
target_link_libraries(occ_history_probe PRIVATE ${OpenCASCADE_LIBRARIES})
