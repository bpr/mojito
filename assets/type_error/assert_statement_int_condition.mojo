# expect: no overload matches the supplied arguments
# An `assert` condition is a `Bool` argument of `debug_assert`: an `Int`
# matches no overload, as at the pin.

def main():
    var n = 2
    assert n
    print("ok")
