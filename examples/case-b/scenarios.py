#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Simulated capture-device scenarios for Case B (no hardware access).

Modes:
  normal          device -> bounded queue -> estimator, all samples delivered
  slow_consumer   consumer is slower than the producer, delivery still exact
  queue_saturation bounded queue overflows; drops are reported, never hidden
  cancel          consumer stops after a cancellation token; pending is reported
  disconnect      the simulated device disconnects mid-stream

Every mode writes one JSON object to stdout. The test harness treats the JSON
as data, not as a success claim: counts and error types are asserted separately.
"""
import argparse
import json
import queue
import threading
import time


def device(samples, disconnect_at=None):
    for index in range(samples):
        if disconnect_at is not None and index == disconnect_at:
            raise ConnectionError("simulated device disconnected")
        yield {"index": index, "voltage_v": 1.0 + (index % 8) * 0.125}


def estimate(previous, sample, alpha=0.5):
    return alpha * sample + (1.0 - alpha) * previous


def run_queue(samples, queue_size, consumer_delay_s, disconnect_at=None):
    work = queue.Queue(maxsize=queue_size)
    state = {"produced": 0, "blocked": 0, "consumed": 0, "dropped": 0, "error": None}
    stop = threading.Event()

    def producer():
        try:
            for item in device(samples, disconnect_at):
                # A bounded queue applies backpressure. The producer waits for
                # capacity and never overwrites an item, so this is not a
                # silent drop; blocked counts the observed backpressure.
                if work.full():
                    state["blocked"] += 1
                work.put(item)
                state["produced"] += 1
        except ConnectionError as error:
            state["error"] = str(error)
        finally:
            work.put(None)

    def consumer():
        previous = 0.0
        while not stop.is_set():
            item = work.get()
            if item is None:
                break
            previous = estimate(previous, item["voltage_v"])
            state["consumed"] += 1
            if consumer_delay_s:
                time.sleep(consumer_delay_s)

    producer_thread = threading.Thread(target=producer)
    consumer_thread = threading.Thread(target=consumer)
    producer_thread.start()
    consumer_thread.start()
    producer_thread.join()
    consumer_thread.join()
    return state



def run_cancel(total, limit, queue_size=4):
    work = queue.Queue(maxsize=queue_size)
    state = {"produced": 0, "consumed": 0, "cancelled": False, "error": None}
    cancel = threading.Event()

    def producer():
        for item in device(total):
            while not cancel.is_set():
                try:
                    work.put(item, timeout=0.001)
                    state["produced"] += 1
                    break
                except queue.Full:
                    continue
            if cancel.is_set():
                break
            time.sleep(0.001)

    def consumer():
        previous = 0.0
        while state["consumed"] < limit:
            item = work.get()
            previous = estimate(previous, item["voltage_v"])
            state["consumed"] += 1
        cancel.set()
        state["cancelled"] = True

    producer_thread = threading.Thread(target=producer)
    consumer_thread = threading.Thread(target=consumer)
    producer_thread.start()
    consumer_thread.start()
    consumer_thread.join()
    producer_thread.join()
    state["pending"] = state["produced"] - state["consumed"]
    return state

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["normal", "slow_consumer", "queue_saturation", "cancel", "disconnect"])
    parser.add_argument("--samples", type=int, default=32)
    parser.add_argument("--queue-size", type=int, default=4)
    args = parser.parse_args()

    if args.mode == "normal":
        state = run_queue(args.samples, args.queue_size, 0.0)
        state["expect"] = {"produced": args.samples, "consumed": args.samples, "error": None}
    elif args.mode == "slow_consumer":
        state = run_queue(args.samples, args.queue_size, 0.002)
        state["expect"] = {"produced": args.samples, "consumed": args.samples, "error": None}
    elif args.mode == "queue_saturation":
        state = run_queue(400, 4, 0.001)
        state["expect"] = {
            "produced": 400,
            "consumed": 400,
            "backpressure_observed": state["blocked"] > 0,
            "silent_drop": False,
        }
    elif args.mode == "cancel":
        # The consumer owns the cancellation token: it processes eight items
        # and stops; the producer is not allowed to rewrite the backlog.
        state = run_cancel(args.samples, 8, args.queue_size)
        state["expect"] = {"cancelled": True, "consumed": 8}
    else:
        state = run_queue(args.samples, args.queue_size, 0.0, disconnect_at=10)
        state["expect"] = {
            "produced": 10,
            "consumed": 10,
            "error": "simulated device disconnected",
        }

    state["mode"] = args.mode
    state["device"] = "simulated"
    print(json.dumps(state, sort_keys=True))


if __name__ == "__main__":
    main()
