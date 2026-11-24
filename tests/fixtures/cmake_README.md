# CMake output fixtures

Captured with CMake 3.25.1, GCC 12.2.0 and Ninja 1.11.1 on Debian 12.
The local sample builds a static library from 16 C++ translation units and runs
eight `check_cxx_source_compiles` probes during configuration. `part1.cpp` contains
a deliberate `#warning`; `part8.cpp` refers to `missing_symbol` for the failed build.
The raw files retain the captured stdout and stderr, including compiler locations.

- `cmake_configure_raw.txt`: `cmake -S source -B make-build -G 'Unix Makefiles'`
- `cmake_build_raw.txt`: `cmake --build make-build -- -w`
- `cmake_failed_build_raw.txt`: `cmake --build make-build --verbose`, exit 2
- `cmake_ninja_build_raw.txt`: `cmake --build ninja-build`, after configuring with `-G Ninja`
