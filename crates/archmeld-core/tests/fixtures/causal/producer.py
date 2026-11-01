"""Producer side of the B016 asynchronous handoff fixture.

The producer releases one message with an explicit correlation id and then
waits for the consumer process to acknowledge it. The trace is written by the
real run, not by the importer.
"""

import json
import os
import subprocess
import sys
import threading
import time

trace_path = sys.argv[1]
worker_path = sys.argv[2]
correlation = "order-42"


def produce():
    start = time.perf_counter_ns()
    payload = {"correlation_id": correlation, "item": 7}
    time.sleep(0.02)
    end = time.perf_counter_ns()
    return payload, start, end


payload, producer_start, producer_end = produce()

worker = subprocess.Popen(
    [sys.executable, worker_path],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
)
child_out, child_err = worker.communicate(json.dumps(payload) + "\n", timeout=20)
if worker.returncode != 0:
    sys.stderr.write(child_err)
    raise SystemExit(worker.returncode)
consumer = json.loads(child_out.strip().splitlines()[-1])
if consumer["correlation_id"] != correlation:
    raise SystemExit("correlation id changed between producer and consumer")

helper_start = time.perf_counter_ns()
time.sleep(0.01)
helper_end = time.perf_counter_ns()

trace = {
    "spans": [
        {
            "span_id": "producer",
            "parent_span_id": None,
            "name": "produce",
            "kind": "function",
            "clock_id": "clock-parent",
            "start_ns": str(producer_start),
            "end_ns": str(producer_end),
            "process_id": str(os.getpid()),
            "thread_id": str(threading.get_ident()),
            "attributes": {"archmeld.probe_id": "probe.producer"},
        },
        {
            "span_id": "consumer",
            "parent_span_id": None,
            "name": "consume",
            "kind": "function",
            "clock_id": "clock-child",
            "start_ns": consumer["start_ns"],
            "end_ns": consumer["end_ns"],
            "process_id": str(worker.pid),
            "thread_id": "1",
            "attributes": {"archmeld.probe_id": "probe.consumer"},
        },
        {
            "span_id": "helper",
            "parent_span_id": None,
            "name": "helper",
            "kind": "function",
            "clock_id": "clock-parent",
            "start_ns": str(helper_start),
            "end_ns": str(helper_end),
            "process_id": str(os.getpid()),
            "thread_id": str(threading.get_ident()),
            "attributes": {"archmeld.probe_id": "probe.helper"},
        },
    ],
    "links": [
        {
            "link_id": "link.message." + correlation,
            "source_span_id": "producer",
            "target_span_id": "consumer",
            "kind": "message",
            "correlation_id": correlation,
            "attributes": {"channel": "stdin-pipe"},
        }
    ],
}
with open(trace_path, "w", encoding="utf-8") as handle:
    json.dump(trace, handle, sort_keys=True)
print(
    json.dumps(
        {
            "correlation_id": correlation,
            "consumer_pid": worker.pid,
            "item": consumer["item"],
        },
        sort_keys=True,
    )
)
