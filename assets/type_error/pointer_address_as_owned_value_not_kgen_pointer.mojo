# `__get_address_as_owned_value` takes a `!kgen.pointer`; a `!lit.ref` is
# not one.
# expect: operand must have '!kgen.pointer<T>' type, not 'ref[
def main():
    var x = 1
    var r = __get_mvalue_as_litref(x)
    var y = __get_address_as_owned_value(r)
    print(y)
