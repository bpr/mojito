# Thin lambda forms: annotated local bindings, immediate invocation, omitted
# argument list, and the minimal `lambda: None`.
def main():
    var double: def(x: Int) -> Int = lambda (x: Int) {} -> Int: x * 2
    print(double(21))
    print((lambda (x: Int) {} -> Int: x + 1)(2))
    var answer: def() -> Int = lambda -> Int: 42
    print(answer())
    var nothing: def() = lambda: None
    nothing()
    # Nesting: the inner lambda captures the outer's argument through its own
    # capture list; the outer lambda itself stays thin.
    var nested: def(x: Int) -> Int = lambda (x: Int) -> Int: (lambda (y: Int) {imm x} -> Int: y + x)(3)
    print(nested(6))
