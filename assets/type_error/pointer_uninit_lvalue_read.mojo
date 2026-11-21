# `__get_address_as_uninit_lvalue(k)` names an uninitialized place, which an
# assignment initializes; reading it is a use of an uninitialized value.
# expect: use of uninitialized value '(reference value)'
def main():
    var x = 1
    var y = __get_address_as_uninit_lvalue(Pointer(to=x)._mlir_value)
    print(y)
