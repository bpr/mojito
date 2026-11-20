# expect: value passed to 'o': expected MutOrigin, found ImmOrigin
# A call bracket's origin argument converts to its binder's declared
# `Origin[mut=…]` type, so an `ImmOrigin(o)` cannot fill a `MutOrigin`
# binder. Both compilers reject (pinned Mojo: "value passed to 'o' cannot
# be converted from 'ImmOrigin' to 'MutOrigin'").
def put[o: MutOrigin](p: Pointer[Int, o]):
    p[] = 5


def main():
    var a = 1
    put[ImmOrigin(origin_of(a))](Pointer(to=a))
    print(a)
