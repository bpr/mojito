# expect: failed to infer parameter 'origin_of(_mlir_origin)'
# A bracket fixing an origin binder's `mut=` binder still leaves the
# origin itself to the arguments, which name none.
def f[m: Bool, //, o: Origin[mut=m]](x: Int) -> Int:
    return x


def main():
    print(f[m=True](1))
