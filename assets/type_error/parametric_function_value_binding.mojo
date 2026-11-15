# A bare reference to a parametric `def` binds none of its parameters, so
# it is no runtime value: the pin names the first unbound parameter.
# expect: cannot use parametric function as a runtime closure: parameter 'T' of type 'Intable' is not bound
def as_int[T: Intable](x: T) -> Int:
    return Int(x)


def main():
    var g = as_int
    print(g(3))
