# expect: expression must be mutable in assignment ('x')
# The `rebind` overload is selected on the declaration: a parameter bounded
# by `TrivialRegisterPassable` is rebound by value even though the
# `T == Int` arm only ever runs with `x` an `Int`.
def put[T: TrivialRegisterPassable](mut x: T):
    comptime if T == Int:
        rebind[Int](x) = 7

def main():
    var v = 3
    put[Int](v)
    print(v)
