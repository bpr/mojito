# A free function's `ref[o]` result over its own origin binder `o`, bound
# from a pointer argument's provenance, names the pointee's storage, so the
# result's loan keeps that storage alive until the result is read: through
# a `Pointer`, its `!lit.ref`, a `String` pointee, and a `ref` binding.
def read[o: ImmOrigin](r: Pointer[Int, o]) -> ref[o] Int:
    return r[]


def read_lit[o: ImmOrigin](r: Pointer[Int, o]._mlir_lit_ref) -> ref[o] Int:
    return __get_litref_as_mvalue(r)


def read_string[o: ImmOrigin](r: Pointer[String, o]) -> ref[o] String:
    return r[]


def pointer_result():
    var x = 222
    print(read(Pointer(to=x)))


def lit_ref_result():
    var x = 223
    print(read_lit(__get_mvalue_as_litref(x)))


def string_result():
    var x = String("hello!")
    print(read_string(Pointer(to=x)))


def bound_result():
    var x = 224
    ref y = read(Pointer(to=x))
    print(y)


def main():
    pointer_result()
    lit_ref_result()
    string_result()
    bound_result()
