# expect: invalid call to 'put': value passed to 'o' cannot be converted from 'ImmOrigin' to 'MutOrigin'
# A method bracket's origin argument converts to its binder's declared
# `Origin[mut=…]` type, so an `ImmOrigin(o)` cannot fill a `MutOrigin`
# binder. Both compilers reject (pinned Mojo: "value passed to 'o' cannot
# be converted from 'ImmOrigin' to 'MutOrigin'").
struct S:
    def __init__(out self):
        pass

    def put[o: MutOrigin](self, p: Pointer[Int, o]):
        p[] = 5


def main():
    var a = 1
    var s = S()
    s.put[ImmOrigin(origin_of(a))](Pointer(to=a))
    print(a)
