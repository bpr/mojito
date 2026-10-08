# expect: cannot use a dynamic value in comptime initializer
def f(n: Int) -> Int:
    return n + 1


def main():
    var y = 3
    comptime x = f(y)
    print(x)
