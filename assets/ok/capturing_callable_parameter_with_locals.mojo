# A callable parameter bound to a capturing closure, in a body that binds its
# own locals before calling it. The closure's environment reaches the callee
# only at runtime, and the parameter's slot sits behind those locals — so the
# native instance takes the closure as a trailing runtime parameter and the
# call stays indirect, while the VM keeps calling the closure it bound.
def accumulate[
    origins: OriginSet, //, f: def(x: Int) capturing[origins] -> Int
](value: Int) -> Int:
    var bump = 1
    var total = f(value) + bump
    return total

def main():
    var factor = 3
    var offset = 10

    @parameter
    def scale(x: Int) -> Int:
        return x * factor + offset

    print(accumulate[scale](5))
    factor = 2
    print(accumulate[scale](5))
