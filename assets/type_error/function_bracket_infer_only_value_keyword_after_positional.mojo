# expect: unexpected keyword parameter 'b'
# An infer-only value binder keyed after a positional argument is
# unexpected, though one keyed ahead of it (`a=1`) binds.
def g[a: Int, b: Int, //, T: AnyType]():
    print(a, b)


def main():
    g[a=1, Int, b=2]()
