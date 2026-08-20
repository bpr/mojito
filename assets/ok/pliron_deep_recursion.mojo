def sum_down(n: Int) -> Int:
    if n == 0:
        return 0
    return n + sum_down(n - 1)

def compute() -> Int:
    return sum_down(5000)

def main():
    print(compute())
