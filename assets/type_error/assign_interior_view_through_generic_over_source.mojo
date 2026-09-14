# A generic identity over the view keeps its origin, so the `String`
# initializer's argument still borrows `s`'s owned bytes. The pinned Mojo
# rejects it with the same text.
# expect: aliasing values passed immutably to 'args' argument and constructed as a result in 'String' initializer call
def id_view[o: ImmOrigin](v: StringSpan[o]) -> StringSpan[o]:
    return v

def main():
    var s = String("abc  ")
    s = String(id_view(s.rstrip()))
    print(s)
