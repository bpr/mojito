# expect: type mismatch for variable 'x': expected Int, found StringLiteral
# A type error in the untaken arm of a `comptime if` is reported, as the
# pinned Mojo reports it: every arm is checked with the declaration's
# parameters symbolic before elaboration selects `n == 0`.
def f[n: Int]() -> Int:
    comptime if n == 0:
        return 1
    else:
        var x: Int = "hello"
        return x


def main():
    print(f[0]())
