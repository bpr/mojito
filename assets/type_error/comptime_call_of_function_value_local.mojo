# expect: cannot use a dynamic value in comptime initializer
# A `var` holding a function is a run-time value, even over a nested `def`
# that captures nothing.


def main():
    def mk() -> Int:
        return 7

    var f = mk
    comptime t = f()
    print(t)
