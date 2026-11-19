# A bare reference to a function leaving its origin's `mut=` binder
# unbound is no runtime value: the pin names that binder.
# expect: cannot use parametric function as a runtime closure: parameter 'm' of type 'Bool' is not bound
def peek[m: Bool, //, o: Origin[mut=m]](p: Pointer[Int, o]) -> Int:
    return p[]


def main():
    var x = 3
    var g = peek
    print(g(Pointer(to=x)))
