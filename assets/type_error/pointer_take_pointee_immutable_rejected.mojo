# expect: invalid call to 'unsafe_take_pointee': violated constraint
# `Pointer.unsafe_take_pointee` is declared `where Self.mut and ...`, so a
# take through an `as_imm()` pointer violates its constraint, as at the pin.
def main():
    var x = 1
    var p = Pointer(to=x).as_imm()
    var v = p.unsafe_take_pointee()
    print(v)
