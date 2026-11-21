# expect: aliasing values passed mutably to 'p' argument and passed mutably to 'q' argument
# A `Pointer[T, _]` parameter's origin is an infer-only binder each call
# binds to its argument's own origin, so two arguments pointing at one place
# alias, as at the pin.
def same(p: Pointer[Int, _], q: Pointer[Int, _]):
    print(p[], q[])

def main():
    var x = 1
    var p = Pointer(to=x)
    same(p, p)
