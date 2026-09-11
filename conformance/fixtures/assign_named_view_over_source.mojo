# A call whose argument is a named view of `s` (`s.rstrip()` is a view at
# upstream's `origin_of(s)._get_owned_interior["bytes"]`), assigned straight
# back to `s`. The pinned Mojo rejects it ("aliasing values passed immutably
# to 'args' argument and constructed as a result in 'String' initializer
# call"); Mojito's loan ends at the view's last use, before the store, so
# it accepts and prints `abc`.
def main():
    var s = String("abc  ")
    var r = s.rstrip()
    s = String(r)
    print(s)
