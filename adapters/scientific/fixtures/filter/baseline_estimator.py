# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""IIR estimator with explicit physical units and state order.

units: input V, output V, alpha dimensionless, sample rate Hz, phase rad
shape: one-dimensional float64 arrays
state: previous output in V
update order: read x[n], compute y[n] = alpha*x[n] + (1-alpha)*y[n-1],
              append y[n], then make y[n] the next previous value.
"""

import math


def run_filter(samples, alpha, initial=0.0):
    output = []
    previous = initial
    for sample in samples:
        current = alpha * sample + (1.0 - alpha) * previous
        output.append(current)
        previous = current
    return output


def wrap_phase(angle):
    wrapped = angle
    while wrapped >= math.pi:
        wrapped -= 2.0 * math.pi
    while wrapped < -math.pi:
        wrapped += 2.0 * math.pi
    return wrapped
