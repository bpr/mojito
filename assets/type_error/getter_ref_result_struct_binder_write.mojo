# expect: reference returned by '__getitem__' whose origin mutability is not known here
# A store through a getter's `ref[Self.o]` result over the struct's
# `Origin[mut=m]` binder rejects in the method body, as at the pin: `m` is
# not provably `True` there, however a caller binds it.
struct R[mut: Bool, //, o: Origin[mut=mut]](ImplicitlyCopyable):
    var p: Pointer[Int, Self.o]
    def __init__(out self, ref[Self.o] x: Int):
        self.p = Pointer(to=x)
    def __getitem__(self) -> ref[Self.o] Int:
        return self.p[]

struct View[m: Bool, //, o: Origin[mut=m]]:
    var r: R[Self.o]
    def __init__(out self, r: R[Self.o]):
        self.r = r
    def put(mut self, x: Int):
        self.r[] = x

def main():
    var x = 1
    var v = View(R(x))
    v.put(9)
    print(x)
