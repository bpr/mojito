def classify(n: Int) -> Int:
    if n < 0:
        return -1
    elif n == 0:
        return 0
    else:
        return 1


def compute() -> Int:
    return classify(-7) * 100 + classify(0) * 10 + classify(42)


def main():
    print(compute())
