# expect: failed to infer parameter 'origin_of(_mlir_origin)'
# The argument binds `o`, and with it the shared `mut=` binder `m`, but
# no argument names the sibling `t`.
def f[m: Bool, //, o: Origin[mut=m], t: Origin[mut=m]](p: Pointer[Int, o]):
    print(p[])


def main():
    var y = 4
    f(Pointer(to=y))
