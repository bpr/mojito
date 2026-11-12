# expect: dynamic type values not permitted yet; try creating a 'comptime' instead of a 'var'
# A type binder has no run-time value: it binds through `comptime`.
def g[T: Copyable]():
    var y = T


def main():
    g[Int]()
