// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0
//
// Real Tracy client probe: emits nested zones and a plot, then exits so
// tracy-capture writes a native .tracy file. Test evidence only.

#include <chrono>
#include <thread>

#include "tracy/Tracy.hpp"

int main() {
  {
    ZoneScopedN("stage.queue");
    TracyPlot("queue_depth", int64_t{0});
    std::this_thread::sleep_for(std::chrono::milliseconds(30));
    TracyPlot("queue_depth", int64_t{3});
    {
      ZoneScopedN("stage.compute");
      std::this_thread::sleep_for(std::chrono::milliseconds(30));
    }
    TracyPlot("queue_depth", int64_t{1});
    std::this_thread::sleep_for(std::chrono::milliseconds(30));
    TracyPlot("queue_depth", int64_t{0});
  }
  FrameMark;
  std::this_thread::sleep_for(std::chrono::milliseconds(200));
  return 0;
}
