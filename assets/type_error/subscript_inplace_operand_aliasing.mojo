# expect: borrowed mutably
# `xs[1] += xs[0]` lends `xs` to `__iadd__`'s `mut self` and, through the
# operand's `StringSpan` view, to its read `other` at once.
def main():
    var xs: List[String] = ["x", "y"]
    xs[1] += xs[0]
    print(xs[1])
