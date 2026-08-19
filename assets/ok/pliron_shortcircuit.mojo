def check(a: Bool, b: Bool) -> Int:
    if a and b:
        return 4
    if a or b:
        return 2
    if not a:
        return 1
    return 0


def compute() -> Int:
    var mixed = check(True, True) * 1000 + check(True, False) * 100 + check(False, True) * 10
    return mixed + check(False, False)


def main():
    print(compute())
