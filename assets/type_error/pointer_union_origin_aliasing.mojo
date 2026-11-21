# expect: aliasing values passed mutably to 'a' argument and passed mutably to 'b' argument in 'bump' call
# A pointer whose origin is the union `origin_of(x, y)` carries both members,
# so passing it beside `mut y` aliases, as at the pin.
def bump(mut a: Int, b: Pointer[Int, _]):
    a += b[]

def main():
    var x = 5
    var y = 6
    var p = Pointer[Int, origin_of(x, y)](to=x)
    bump(y, p)
    print(y)
