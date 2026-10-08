# expect: type mismatch for comptime 'D'
# An annotated module constant that applies a callable is typed where it is
# declared, though it waits for its first demand: an `Int` result does not
# implicitly convert to a `Float64` annotation.
def f(x: Int) -> Int:
    return x + 1

comptime D: Float64 = f(2)

def main():
    print(D)
