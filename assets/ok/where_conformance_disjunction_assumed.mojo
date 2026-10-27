# A caller's own `where` clause discharges the same clause at its callee: an
# unknown conformance stays the `conforms_to` proposition, so the two
# disjunctions are one.
def g[T: AnyType]() where conforms_to(T, Copyable) or conforms_to(T, Movable):
    print("g")

def f[T: AnyType]() where conforms_to(T, Copyable) or conforms_to(T, Movable):
    g[T]()

def main():
    f[Int]()
