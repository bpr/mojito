# A `ref` binding of a returned reference names the argument's origin and
# borrows nothing exclusively: a write to the owner while it lives runs, as
# at the pin.
def borrow(ref value: Int) -> ref[value] Int:
    return value

def main():
    var value = 1
    ref view = borrow(value)
    value = 2
    print(view)
