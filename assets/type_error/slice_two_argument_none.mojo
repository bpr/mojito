# expect: no constructor overload matches
# `Slice`'s two-argument constructor takes `Int` bounds, and its
# three-argument one needs `step`, so `Slice(None, 3)` matches neither.
def main():
    var a = Slice(None, 3)
    print(a)
