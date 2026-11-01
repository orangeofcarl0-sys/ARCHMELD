// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0
// Simulated capture device: deterministic samples, no hardware access.
#include <cmath>
#include <cstdio>
#include <cstdlib>

int main(int argc, char** argv) {
    if (argc != 2) {
        std::fprintf(stderr, "usage: capture <output.csv>\n");
        return 2;
    }
    FILE* out = std::fopen(argv[1], "wb");
    if (!out) {
        std::perror("capture output");
        return 1;
    }
    std::fprintf(out, "sample,voltage_v\n");
    for (int index = 0; index < 32; ++index) {
        const double voltage = 1.0 + 0.25 * std::sin(index / 4.0);
        std::fprintf(out, "%d,%.9f\n", index, voltage);
    }
    std::fclose(out);
    return 0;
}
