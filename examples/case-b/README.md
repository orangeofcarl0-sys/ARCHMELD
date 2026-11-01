# Case B · C++ capture → Python estimator → UI

This is the AT066 reference case. The C++ program is a **simulated capture
device** (no hardware access); it writes deterministic voltage samples. The
Python estimator consumes that real captured CSV, and the UI renders the real
estimate as static HTML with no scripts or remote assets.

The acceptance test builds and runs the C++ binary, records baseline numeric
output, freezes a BehaviorContract, refactors only the estimator through the
staged apply gate, verifies the same output against the C++ capture input,
imports a real trace with source entity ids, and exports an offline review.
