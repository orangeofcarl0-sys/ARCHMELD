package main

import (
	"fmt"

	u "example.com/langpack/util"
)

type Runner interface {
	Run() int
}

type Job struct{}

func (j Job) Run() int { return u.Helper(2) }

func callViaInterface(r Runner) int { return r.Run() }

func callViaFunc(f func(int) int, v int) int { return f(v) }

func main() {
	fmt.Println(u.Helper(1), callViaInterface(Job{}), callViaFunc(u.Helper, 3))
}
