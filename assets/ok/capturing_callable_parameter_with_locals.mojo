# A closure-trait binder bound to a capture-list closure, in a body that
# binds its own locals before calling it: the closure's storage reaches the
# callee as a run-time argument behind those locals, and a write to a
# captured local between calls is seen by the next one.
def accumulate[F: def(x: Int) -> Int](f: F, value: Int) -> Int:
    var bump = 1
    var total = f(value) + bump
    return total

def main():
    var factor = 3
    var offset = 10

    def scale(x: Int) {imm factor, imm offset} -> Int:
        return x * factor + offset

    print(accumulate(scale, 5))
    factor = 2
    print(accumulate(scale, 5))
