# expect: cannot call raising function in comptime initializer
def f(n: Int) raises -> Int:
    return n + 1


def main():
    comptime x = f(1)
    print(x)
