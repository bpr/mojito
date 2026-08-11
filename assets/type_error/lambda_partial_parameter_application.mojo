# Value-base compile-time parameter application outside an immediate call is
# the documented `objs[0](args)` gap: a generic lambda cannot be partially
# applied into a stored function value.
# expect: cannot be indexed here
def apply(f: def(x: Int) capturing[_] -> Int, v: Int) -> Int:
    return f(v)

def main():
    print(apply((lambda [N: Int](x: Int) {} -> Int: x + N)[5], 3))
