# expect: is not concrete, use '[]' to bind missing parameters
# An `address_space=_` hole is solved only from an initializer or a call's
# argument; an uninitialized local has neither (upstream: "'Pointer[Int,
# origin_of(x), address_space=_]' is not concrete, use '[]' to bind missing
# parameters").
def main():
    var x = 42
    var p: Pointer[Int, origin_of(x), address_space=_]
    p = Pointer(to=x)
    print(p[])
