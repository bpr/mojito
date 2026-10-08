# expect: type mismatch for argument 'x' to 'f'
# A module constant that applies a callable is typed where it is declared,
# though nothing reads it and it is never evaluated.
def f(x: Int) -> Int:
    return x + 1

comptime W = f("x")

def main():
    print(1)
