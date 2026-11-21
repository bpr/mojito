# `__get_address_as_uninit_lvalue` takes a `!kgen.pointer`; an `Int` place
# is not one.
# expect: operand must have '!kgen.pointer<T>' type, not 'Int'
def main():
    var x = 1
    __get_address_as_uninit_lvalue(x) = 3
    print(x)
