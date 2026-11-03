# A lambda with a capture list is a closure, which does not convert to a
# run-time `def(...)` parameter; `{imm}` makes it a closure even with nothing
# captured.
# expect: type mismatch for argument 'f' to 'transform'
def transform(f: def(x: Int) -> Int, value: Int) -> Int:
    return f(value)

def main():
    print(transform(lambda (x: Int) {imm} -> Int: x + 1, 4))
