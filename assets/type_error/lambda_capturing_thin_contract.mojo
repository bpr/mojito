# A lambda that captures is a closure and cannot bind to an unqualified
# `def(...)` contract; `{imm}` makes it a closure even with nothing captured.
# expect: must spell 'capturing[...]'
def transform(f: def(x: Int) -> Int, value: Int) -> Int:
    return f(value)

def main():
    print(transform(lambda (x: Int) {imm} -> Int: x + 1, 4))
