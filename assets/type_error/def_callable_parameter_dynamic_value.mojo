# A `def`'s callable-value parameter takes a compile-time function: a
# run-time local holding a function is the pin's dynamic-value rejection.
# expect: cannot use a dynamic value in a parameter list


def seven() -> Int:
    return 7


def g[f: def() thin -> Int]() -> Int:
    return f()


def main():
    var h = seven
    print(g[h]())
