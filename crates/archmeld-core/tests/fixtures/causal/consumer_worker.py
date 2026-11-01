"""Consumer process for the B016 asynchronous handoff fixture."""

import json
import os
import sys
import time

payload = json.loads(sys.stdin.readline())
# The consumer starts after the producer released the message; the only thing
# that ties the two spans together is the correlation id it echoes back.
time.sleep(0.03)
start = time.perf_counter_ns()
item = payload["item"] * 2
end = time.perf_counter_ns()
sys.stdout.write(
    json.dumps(
        {
            "correlation_id": payload["correlation_id"],
            "item": item,
            "start_ns": str(start),
            "end_ns": str(end),
            "process_id": os.getpid(),
        },
        sort_keys=True,
    )
    + "\n"
)
sys.stdout.flush()
