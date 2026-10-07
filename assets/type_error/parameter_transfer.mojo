# expect: cannot transfer from a parameter expression
# A compile-time parameter is a value, not storage: `p^` has nothing to move
# out of, whatever the parameter's type, as at the pin.
def take(var t: Int):
    print(t)


def g[p: Int]():
    take(p^)


def main():
    g[1]()
