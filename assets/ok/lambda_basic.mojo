# Thin lambda forms: annotated local bindings (spelled `def(...) thin`, the
# only storable local callable annotation), immediate invocation, omitted
# argument list, and the minimal `lambda: None`.
def main():
    var double: def(x: Int) thin -> Int = lambda (x: Int) {} -> Int: x * 2
    print(double(21))
    print((lambda (x: Int) {} -> Int: x + 1)(2))
    var answer: def() thin -> Int = lambda -> Int: 42
    print(answer())
    var nothing: def() thin = lambda: None
    nothing()
    # Nesting: the inner lambda captures the outer's argument through its own
    # capture list; the outer lambda itself stays thin.
    var nested: def(x: Int) thin -> Int = lambda (x: Int) -> Int: (lambda (y: Int) {imm x} -> Int: y + x)(3)
    print(nested(6))
