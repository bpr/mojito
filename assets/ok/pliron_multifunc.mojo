def square(n: Int) -> Int:
    return n * n


def cube(n: Int) -> Int:
    return square(n) * n


def poly(x: Int, a: Int, b: Int, c: Int) -> Int:
    return a * cube(x) + b * square(x) + c


def compute() -> Int:
    return poly(7, 3, -2, 11) + poly(-4, 1, 0, 5)


def main():
    print(compute())
