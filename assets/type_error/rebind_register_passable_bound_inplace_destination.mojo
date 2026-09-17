# expect: expression must be mutable for in-place operator destination ('x')
# The `rebind` overload is selected on the declaration: a parameter bounded
# by `TrivialRegisterPassable` is rebound by value even though the
# `T == Int` arm only ever runs with `x` an `Int`.
def bump[T: TrivialRegisterPassable](mut x: T):
    comptime if T == Int:
        rebind[Int](x) += 1

def main():
    var v = 3
    bump[Int](v)
    print(v)
