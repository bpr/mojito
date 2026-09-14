# A call whose argument is a named view of `s` (`s.rstrip()` is a view at
# upstream's `origin_of(s)._get_owned_interior["bytes"]`), assigned straight
# back to `s`. The aliasing rule is judged by origin, not by the view's
# lifetime, so both the pinned Mojo and Mojito reject it ("aliasing values
# passed immutably to 'args' argument and constructed as a result in
# 'String' initializer call").
def main():
    var s = String("abc  ")
    var r = s.rstrip()
    s = String(r)
    print(s)
