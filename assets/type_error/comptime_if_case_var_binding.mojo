# expect: 'var' bindings are not supported in 'comptime match'
# A `comptime if case` pattern binds compile-time values; a `var` binding is
# the pin's `comptime match` rejection.
def f[n: Int]():
    comptime if case var x = n:
        print(x)


def main():
    f[3]()
