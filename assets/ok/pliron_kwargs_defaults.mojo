def blend(a: Int, b: Int = 10, c: Int = 100) -> Int:
    return a + 2 * b + 3 * c

def scale(x: Float64, factor: Float64 = 0.5) -> Float64:
    return x * factor

def compute() -> Int:
    var total = blend(1)
    total = total + blend(1, c = 2)
    total = total + blend(c = 1, a = 2, b = 3)
    total = total + blend(4, 5)
    if scale(4.0) == 2.0:
        total = total + 1000
    return total

def main():
    print(compute())
