# expect: aliasing values passed mutably to 'b' argument and passed immutably to 'rest' argument in 'r' call
# A pack element read by borrow is held by reference even when its type is
# trivial, so a mutable place passed to a `ref` parameter conflicts with the
# same place gathered into the pack, as at the pin. (With a regular `c: Int`
# in place of the pack the read takes a copy and the call is accepted.)
def r[*Ts: Writable](ref b: Int, *rest: *Ts) -> Int:
    return b


def main():
    var x = 1
    print(r(x, x))
