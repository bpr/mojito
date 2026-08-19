def fib(n: Int) -> Int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


def compute() -> Int:
    return fib(15)


def main():
    print(compute())
