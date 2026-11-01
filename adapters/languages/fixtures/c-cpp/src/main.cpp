#include "shape.h"

double apply(double (*fn)(double), double value) { return fn(value); }

double scale_value(double value) { return SCALE(value); }

double measure(const Shape& shape) { return shape.area(); }
