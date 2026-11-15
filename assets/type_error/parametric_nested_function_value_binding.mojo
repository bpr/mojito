# A nested parametric `def` read as a value is a parametric function, not
# an escaping closure.
# expect: cannot use parametric function as a runtime closure: parameter 'U' of type 'ImplicitlyCopyable' is not bound
def main():
    def inner[U: ImplicitlyCopyable](x: U) -> U:
        return x

    var g = inner
    print(g(3))
