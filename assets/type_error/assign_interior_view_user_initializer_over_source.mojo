# A user initializer assigned straight back over the struct its argument
# views: `w.t.rstrip()` borrows `w.t`'s owned bytes, so the result would
# alias an argument still being read. The pinned Mojo rejects it with the
# same text.
# expect: aliasing values passed immutably to 'v' argument and constructed as a result in 'W' initializer call
struct W(Movable):
    var t: String

    def __init__(out self, v: StringSpan):
        self.t = String(v)


def main():
    var w = W(StringSpan("abc  "))
    w = W(w.t.rstrip())
    print(w.t)
