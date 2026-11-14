# `__get_litref_as_mvalue` takes a `!lit.ref`, not the place it would refer to.
# expect: operand isn't a '!lit.ref' type 'Int'
def main():
    var x = 1
    print(__get_litref_as_mvalue(x))
