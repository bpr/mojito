# Benchmark: direct calls and recursion.
# Naive recursive Fibonacci plus an iterative call-heavy mixing loop.
def fib(n: Int) -> Int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

def mix(a: Int, b: Int) -> Int:
    return (a * 31 + b) % 1000003

def step(acc: Int, i: Int) -> Int:
    return mix(acc, i) + 1

def main():
    var f: Int = fib(29)

    var acc: Int = 0
    var i: Int = 0
    while i < 600000:
        acc = step(acc, i)
        i += 1

    print("fib29:", f)
    print("acc:", acc)
