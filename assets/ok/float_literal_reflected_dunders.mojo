# Upstream's reflected `FloatLiteral` dunders, each `type_of(rhs OP self)`
# (`__rmod__` is `type_of(rhs.__mod__(self))`).
def main():
    print(2 + 2.5, 1 / 2.0, 3 - 0.5, 2 * 1.5)
    print((2.5).__radd__(1.0), (2.5).__rsub__(1.0), (2.0).__rmul__(1.5), (2.0).__rtruediv__(1.0))
    print((2.0).__rfloordiv__(7.0), (2.0).__rmod__(7.5))
