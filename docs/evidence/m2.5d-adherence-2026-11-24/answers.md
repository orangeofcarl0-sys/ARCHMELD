# M2.5-D final answers (canonicalized)

Each model returned one JSON object. Paths below are repository-relative; the
original reports sometimes used absolute run paths. `candidate` and `unknown`
are the model's own labels; no run used `confirmed` for a call edge.

## A1

```json
{
  "callers": [
    {"function":"list_price","file":"src/pricing.py","line":5,"kind":"direct","status":"candidate"},
    {"function":"summarize_order","file":"src/wrapper.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"checkout","file":"src/checkout.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"check_price","file":"src/analytics.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"build_report","file":"src/reporting.py","line":4,"kind":"indirect","status":"candidate"},
    {"function":"dynamic_discount","file":"src/dynamic.py","line":6,"kind":"dynamic","status":"unknown"}
  ],
  "assumptions": [
    {"file":"src/analytics.py","line":4,"what":"check_price asserts apply_discount(100) == 90"},
    {"file":"src/invoice.py","line":3,"what":"EXPECTED_DISCOUNT = 0.9 is used by invoice_total"}
  ],
  "found_misbinding": false
}
```

## A2

```json
{
  "callers": [
    {"function":"list_price","file":"src/pricing.py","line":5,"kind":"direct","status":"candidate"},
    {"function":"checkout","file":"src/checkout.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"summarize_order","file":"src/wrapper.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"check_price","file":"src/analytics.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"dynamic_discount","file":"src/dynamic.py","line":6,"kind":"dynamic","status":"unknown"},
    {"function":"build_report","file":"src/reporting.py","line":4,"kind":"indirect","status":"candidate"}
  ],
  "assumptions": [
    {"file":"src/analytics.py","line":4,"what":"check_price compares to 90"},
    {"file":"src/invoice.py","line":3,"what":"EXPECTED_DISCOUNT = 0.9 duplicates the old rate"}
  ],
  "found_misbinding": true
}
```

## A3

```json
{
  "callers": [
    {"function":"list_price","file":"src/pricing.py","line":5,"kind":"direct","status":"candidate"},
    {"function":"summarize_order","file":"src/wrapper.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"checkout","file":"src/checkout.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"check_price","file":"src/analytics.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"dynamic_discount","file":"src/dynamic.py","line":5,"kind":"dynamic","status":"unknown"},
    {"function":"build_report","file":"src/reporting.py","line":4,"kind":"indirect","status":"unknown"}
  ],
  "assumptions": [
    {"file":"src/analytics.py","line":4,"what":"check_price compares to 90"},
    {"file":"src/invoice.py","line":3,"what":"EXPECTED_DISCOUNT = 0.9"},
    {"file":"src/invoice.py","line":6,"what":"invoice_total multiplies by the stale constant"}
  ],
  "found_misbinding": true
}
```

A3 reported `build_report` as `unknown` because the graph had no path to the
real function; this is a conservative label, not a wrong caller.

## B1

```json
{
  "callers": [
    {"function":"list_price","file":"src/pricing.py","line":5,"kind":"direct","status":"candidate"},
    {"function":"checkout","file":"src/checkout.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"summarize_order","file":"src/wrapper.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"check_price","file":"src/analytics.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"dynamic_discount","file":"src/dynamic.py","line":5,"kind":"dynamic","status":"unknown"},
    {"function":"build_report","file":"src/reporting.py","line":4,"kind":"indirect","status":"candidate"}
  ],
  "assumptions": [
    {"file":"src/analytics.py","line":4,"what":"check_price compares to 90"},
    {"file":"src/invoice.py","line":3,"what":"EXPECTED_DISCOUNT = 0.9"}
  ],
  "found_misbinding": true
}
```

## B2

```json
{
  "callers": [
    {"function":"pricing.list_price","file":"src/pricing.py","line":5,"kind":"direct","status":"candidate"},
    {"function":"checkout.checkout","file":"src/checkout.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"analytics.check_price","file":"src/analytics.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"wrapper.summarize_order","file":"src/wrapper.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"dynamic.dynamic_discount","file":"src/dynamic.py","line":6,"kind":"dynamic","status":"unknown"},
    {"function":"reporting.build_report","file":"src/reporting.py","line":4,"kind":"indirect","status":"candidate"}
  ],
  "assumptions": [
    {"file":"src/analytics.py","line":4,"what":"check_price compares to 90"},
    {"file":"src/invoice.py","line":3,"what":"EXPECTED_DISCOUNT = 0.9"},
    {"file":"src/wrapper.py","line":4,"what":"* 1.1 is an inferred compensation; 0.9*1.1 was near identity, 0.8*1.1 is not"}
  ],
  "found_misbinding": true
}
```

## B3

```json
{
  "callers": [
    {"function":"src/pricing.py::list_price","file":"src/pricing.py","line":5,"kind":"direct","status":"candidate"},
    {"function":"src/checkout.py::checkout","file":"src/checkout.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"src/wrapper.py::summarize_order","file":"src/wrapper.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"src/analytics.py::check_price","file":"src/analytics.py","line":4,"kind":"direct","status":"candidate"},
    {"function":"src/dynamic.py::dynamic_discount","file":"src/dynamic.py","line":6,"kind":"dynamic","status":"unknown"},
    {"function":"src/reporting.py::build_report","file":"src/reporting.py","line":4,"kind":"indirect","status":"candidate"}
  ],
  "assumptions": [
    {"file":"src/analytics.py","line":4,"what":"check_price compares to 90"},
    {"file":"src/invoice.py","line":3,"what":"EXPECTED_DISCOUNT = 0.9"},
    {"file":"src/invoice.py","line":6,"what":"invoice_total multiplies by the stale constant"},
    {"file":"src/legacy.py","line":1,"what":"a homonym apply_discount (0.5) shadows the name; the index mis-resolves cross-file calls to it"}
  ],
  "found_misbinding": true
}
```
