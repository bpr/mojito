# expect: cannot use a dynamic value in comptime initializer
# A nested `def` that captures a run-time local has no compile-time value.


def main():
    var x = 3

    def mk() {imm x} -> Int:
        return x

    comptime t = mk()
    print(t)
