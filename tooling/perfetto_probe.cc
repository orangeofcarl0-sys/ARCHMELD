// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0
//
// Real Perfetto SDK probe: writes a native .pftrace with nested slices and a
// counter series. Used only to produce test evidence; it is not part of the
// product runtime.

#include <cstdio>
#include <cstdlib>
#include <thread>
#include <string>
#include <vector>

#include "perfetto.h"

PERFETTO_DEFINE_CATEGORIES(perfetto::Category("archmeld"));
PERFETTO_TRACK_EVENT_STATIC_STORAGE();

int main(int argc, char** argv) {
  if (argc != 2) {
    std::fprintf(stderr, "usage: perfetto_probe <output.pftrace>\n");
    return 2;
  }
  perfetto::TracingInitArgs args;
  args.backends = perfetto::kInProcessBackend;
  perfetto::Tracing::Initialize(args);
  perfetto::TrackEvent::Register();

  perfetto::TraceConfig config;
  config.add_buffers()->set_size_kb(4096);
  auto* data_source = config.add_data_sources()->mutable_config();
  data_source->set_name("track_event");
  data_source->set_target_buffer(0);

  auto session = perfetto::Tracing::NewTrace(perfetto::kInProcessBackend);
  session->Setup(config);
  session->StartBlocking();

  {
    TRACE_EVENT_BEGIN("archmeld", "stage.queue", "archmeld.entity_id",
                      std::string("entity.queue"));
    TRACE_STATE("archmeld", "idle", perfetto::StateTrack("queue.state"));
    TRACE_COUNTER("archmeld", "queue_depth", 0);
    std::this_thread::sleep_for(std::chrono::milliseconds(30));
    TRACE_STATE("archmeld", "running", perfetto::StateTrack("queue.state"));
    TRACE_COUNTER("archmeld", "queue_depth", 3);
    TRACE_EVENT_BEGIN("archmeld", "stage.compute");
    std::this_thread::sleep_for(std::chrono::milliseconds(30));
    TRACE_COUNTER("archmeld", "queue_depth", 1);
    TRACE_EVENT_END("archmeld");
    std::this_thread::sleep_for(std::chrono::milliseconds(30));
    TRACE_COUNTER("archmeld", "queue_depth", 0);
    TRACE_STATE("archmeld", "idle", perfetto::StateTrack("queue.state"));
    TRACE_EVENT_END("archmeld");
  }

  session->FlushBlocking();
  session->StopBlocking();
  std::vector<char> trace = session->ReadTraceBlocking();

  FILE* output = std::fopen(argv[1], "wb");
  if (output == nullptr) {
    std::fprintf(stderr, "cannot open output file\n");
    return 2;
  }
  if (!trace.empty() &&
      std::fwrite(trace.data(), 1, trace.size(), output) != trace.size()) {
    std::fprintf(stderr, "short write\n");
    std::fclose(output);
    return 2;
  }
  std::fclose(output);
  return 0;
}
