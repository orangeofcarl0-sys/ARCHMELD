#pragma once

#define SCALE(x) ((x) * FACTOR)

class Shape {
public:
    virtual double area() const = 0;
    virtual ~Shape() = default;
};

class Circle : public Shape {
public:
    explicit Circle(double radius);
    double area() const override;
private:
    double radius_;
};

double apply(double (*fn)(double), double value);
double scale_value(double value);
