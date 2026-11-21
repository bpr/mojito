# A keyword `ref[Self.origin]` constructor argument lends its place to the
# constructed value exactly as a positional one does, so `x` stays live while
# `p` (or a copy of it) is, however the read through it is spelled.


struct P[mut: Bool, //, T: AnyType, origin: Origin[mut=mut]](ImplicitlyCopyable):
    comptime _mlir_lit_ref = __mlir_type[`!lit.ref<`, Self.T, `, `, Self.origin._mlir_origin, `>`]
    comptime _mlir_type = __mlir_type[`!kgen.pointer<`, Self.T, `>`]
    var _mlir_value: Self._mlir_type

    def __init__(out self, *, _mlir_value: Self._mlir_type):
        self._mlir_value = _mlir_value

    def __init__(out self, *, ref[Self.origin] to: Self.T):
        self = Self(_mlir_value=__mlir_op.`lit.ref.to_pointer`(__get_mvalue_as_litref(to)))

    def __getitem__(self, *, k: Int) -> ref[Self.origin] Self.T:
        return self[]

    def __getitem__(self) -> ref[Self.origin] Self.T:
        return __get_litref_as_mvalue(
            __mlir_op.`lit.ref.from_pointer`[_type=Self._mlir_lit_ref, name="(pointer pointee)".value](self._mlir_value)
        )


struct H[o: MutOrigin](ImplicitlyCopyable):
    var r: Pointer[Int, Self.o]

    def __init__(out self, *, ref[Self.o] to: Int):
        self.r = Pointer(to=to)


def idp[T: AnyType, o: MutOrigin](v: P[T, o]) -> P[T, o]:
    return v


def rd[o: MutOrigin](p: P[Int, o]) -> Int:
    return p[]


def rdh[o: MutOrigin](h: H[o]) -> Int:
    return h.r[]


def call_result_receiver():
    var x = 4
    var p = P(to=x)
    print(idp(p)[])


def named_receiver():
    var x = 4
    var p = P(to=x)
    var y = p[]
    print(y)


def passed_view():
    var x = 4
    var p = P(to=x)
    print(rd(p))


def keyword_subscript():
    var x = 4
    var p = P(to=x)
    print(idp(p)[k=1])


def stored_pointer():
    var x = 4
    var h = H(to=x)
    print(rdh(h))


def main():
    call_result_receiver()
    named_receiver()
    passed_view()
    keyword_subscript()
    stored_pointer()
