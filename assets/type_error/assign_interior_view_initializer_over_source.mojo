# A `String(...)` initializer assigned straight back to the String its
# argument views: `s.rstrip()` borrows `s`'s owned bytes
# (`_get_owned_interior["bytes"]`), so the result would alias an argument
# still being read. The pinned Mojo rejects it with the same text.
# expect: aliasing values passed immutably to 'args' argument and constructed as a result in 'String' initializer call
def main():
    var s = String("abc  ")
    s = String(s.rstrip())
    print(s)
