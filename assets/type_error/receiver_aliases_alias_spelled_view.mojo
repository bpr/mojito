# expect: aliasing values passed mutably to 'self' argument and passed immutably to 'string' argument in 'write_string' call
# A view constructed through the `StringSlice` alias borrows the string it
# views, so passing it to a mutating method of that same string aliases the
# receiver, as upstream rejects.
def main():
    var s = String("ab")
    s.write_string(StringSlice(s))
    print(s)
