# expect: call to 'unsafe_write'
# No declared `Pointer.unsafe_write` overload accepts a write through an
# `as_imm()` pointer: each is declared `where Self.mut and ...`. The pin
# reports no matching method (R2156).
def main():
    var x = 1
    var p = Pointer(to=x).as_imm()
    p.unsafe_write(2)
    print(x)
