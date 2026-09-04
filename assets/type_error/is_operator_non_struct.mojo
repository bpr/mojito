# expect: is not defined
# `is` dispatches only to a struct's `__is__`; scalars have no identity
# comparison in Mojito.
def main():
    var n = 1
    print(n is None)
